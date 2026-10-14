//! The VideoToolbox H.264 and HEVC encoders behind `filmcraft_export::VideoEncoder` (macOS; safe code
//! around [`crate::videotoolbox_encode`]).
//!
//! H.264 hardware encoding is opt-in per export (`ExportSettings::hardware_encoding`, `Auto`): its output
//! depends on the machine, while the built-in encoder's is byte-identical everywhere. The factory
//! declines, so the built-in encoder is used, for everything the hardware path does not take: other
//! formats, MXF (Annex B), two-pass VBR, HDR H.264, odd picture sizes, non-square pixels, and any
//! configuration VideoToolbox cannot create a hardware session for. A hardware encoder that fails in
//! the middle of an export reports an error (the export stops); it cannot hand the stream over to
//! another encoder.
//!
//! HEVC (`Format::Hevc`) has no built-in encoder, so choosing the format is the opt-in, the same
//! configurations are declined, and a declined export fails instead of falling back. HEVC Main 10
//! preserves PQ / HLG through 10-bit input; [`hevc_hdr_available`] advertises this capability.

use std::sync::{Once, OnceLock};

use filmcraft_export::{
    BitrateMode, ColorSignal, EncodedPacket, EncoderFrame, ExportError, ExportSettings, FieldOrder, Format, H264Pass, H264Profile, HardwareEncoding,
    VideoEncoder,
};
use filmcraft_isobmff::{AvcConfig, DolbyVisionConfig, FourCc, HevcConfig, SampleEntry};
use filmcraft_time::FrameRate;

use crate::videotoolbox_encode::{ParamSets, VtCodec, VtConfig, VtEncoder, VtPacket, VtProfile, VtRate};

/// The hardware encoder's configuration for an export, or why it is not used for it.
pub fn config_for(w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Result<VtConfig, String> {
    let hevc = s.format == Format::Hevc;
    if s.dolby_vision && (!hevc || s.signal != ColorSignal::HLG || s.sdr) {
        return Err("Dolby Vision Profile 8.4 requires HEVC Main 10 with BT.2020 HLG output".into());
    }
    if !hevc && s.hardware_encoding != HardwareEncoding::Auto {
        return Err("hardware encoding is off".into());
    }
    if s.format.is_mxf() {
        return Err("MXF carries an Annex B stream".into());
    }
    if s.bitrate_mode == BitrateMode::Vbr2Pass || !matches!(s.h264_pass, H264Pass::Single) {
        return Err("two-pass encoding".into());
    }
    if s.bitrate_mode == BitrateMode::Crf {
        return Err("CRF (constant quality)".into());
    }
    let hdr = s.signal.is_hdr();
    if hdr && !hevc {
        return Err("HDR H.264".into());
    }
    if (hdr && !matches!(s.signal, ColorSignal::PQ | ColorSignal::HLG)) || (!hdr && s.signal != ColorSignal::default()) {
        return Err("the colour signal must be BT.709 SDR, or BT.2020 PQ / HLG for HEVC Main 10".into());
    }
    if s.field_order != FieldOrder::Progressive {
        return Err("interlaced output".into());
    }
    if !w.is_multiple_of(2) || !h.is_multiple_of(2) {
        return Err("odd picture size".into());
    }
    if s.pixel_aspect.is_some_and(|(n, d)| n != d) {
        return Err("non-square pixels".into());
    }
    let timescale = u32::try_from(rate.num).ok().filter(|n| *n > 0).ok_or("invalid frame rate")?;
    let frame_duration = u32::try_from(rate.den).ok().filter(|d| *d > 0).ok_or("invalid frame rate")?;
    let kbps = s.bitrate_kbps.max(100);
    let max_kbps = s.max_bitrate_kbps.filter(|m| *m >= kbps).unwrap_or(kbps.saturating_mul(3) / 2);
    let fps = f64::from(timescale) / f64::from(frame_duration);
    Ok(VtConfig {
        width: w,
        height: h,
        timescale,
        frame_duration,
        keyframe_interval: s.keyframe_distance.filter(|k| *k > 0).unwrap_or_else(|| (fps * 2.0).round().max(1.0) as u32),
        profile: match (hevc, s.h264_profile) {
            (true, _) if hdr => VtProfile::HevcMain10,
            (true, _) => VtProfile::HevcMain,
            (false, H264Profile::Baseline) => VtProfile::Baseline,
            (false, H264Profile::Main) => VtProfile::Main,
            (false, H264Profile::High) => VtProfile::High,
        },
        rate: match s.bitrate_mode {
            BitrateMode::Cbr => VtRate::Cbr { kbps },
            _ => VtRate::Vbr { target_kbps: kbps, max_kbps },
        },
    })
}

static HEVC_PROBE: OnceLock<bool> = OnceLock::new();
static HEVC_HDR_PROBE: OnceLock<bool> = OnceLock::new();

/// Whether this machine has a hardware HEVC encoder (asked once, by creating a small session, which
/// takes about 0.1 s): what makes `Format::Hevc` available. [`warm_hevc_probe`] asks it ahead of the
/// first caller.
pub fn hevc_available() -> bool {
    *HEVC_PROBE.get_or_init(|| {
        let probe = VtConfig {
            width: 640,
            height: 360,
            timescale: 30,
            frame_duration: 1,
            keyframe_interval: 30,
            profile: VtProfile::HevcMain,
            rate: VtRate::Vbr { target_kbps: 2000, max_kbps: 3000 },
        };
        let result = VtEncoder::new(probe);
        if let Err(why) = &result {
            log::info!("no hardware HEVC encoder: {why}");
        }
        result.is_ok()
    })
}

/// Whether [`hevc_available`] has its answer yet (diagnostics and tests).
pub fn hevc_probed() -> bool {
    HEVC_PROBE.get().is_some()
}

/// Whether hardware HEVC Main 10 accepts both HLG and PQ with their required colour properties.
/// Only this positive answer permits the export pipeline to send 10-bit HDR pictures.
pub fn hevc_hdr_available() -> bool {
    *HEVC_HDR_PROBE.get_or_init(|| {
        for signal in [ColorSignal::HLG, ColorSignal::PQ] {
            let probe = VtConfig {
                width: 640,
                height: 360,
                timescale: 30,
                frame_duration: 1,
                keyframe_interval: 30,
                profile: VtProfile::HevcMain10,
                rate: VtRate::Vbr { target_kbps: 2000, max_kbps: 3000 },
            };
            if let Err(why) = VtEncoder::with_signal(probe, signal) {
                log::info!("no hardware HEVC Main 10 encoder for {signal:?}: {why}");
                return false;
            }
        }
        true
    })
}

/// Ask [`hevc_available`] now, on a thread of its own, so that the first draw of the format list does
/// not spend its 0.1 s creating a hardware session on the UI thread. Whoever asks while the thread
/// runs waits for the same answer (no longer than asking first); if the thread cannot start, or the
/// probe panics, the first caller asks, as before. Only the first call starts a thread.
pub fn warm_hevc_probe() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new().name("filmcraft-hevc-probe".into()).spawn(|| {
            let _ = std::panic::catch_unwind(|| {
                hevc_available();
                hevc_hdr_available();
            });
        });
        if let Err(why) = spawned {
            log::info!("HEVC probe not started ahead of time: {why}");
        }
    });
}

/// The encoder factory `register` puts in front of the built-in ones: a hardware encoder when the
/// export asks for one (H.264 with `hardware_encoding`; HEVC always) and the OS takes the
/// configuration, `None` (the built-in encoder, if there is one) otherwise.
pub fn videotoolbox_encoder_factory(
    format: Format,
    w: u32,
    h: u32,
    rate: FrameRate,
    s: &ExportSettings,
) -> Option<filmcraft_export::Result<Box<dyn VideoEncoder>>> {
    let name = match format {
        Format::H264 if s.hardware_encoding == HardwareEncoding::Auto => "H.264",
        Format::Hevc => "HEVC",
        _ => return None,
    };
    let config = match config_for(w, h, rate, s) {
        Ok(c) => c,
        Err(why) => {
            log::info!("hardware {name} encoding declined: {why}");
            filmcraft_export::note_hw_encode_declined();
            return if s.dolby_vision || (format == Format::Hevc && s.signal.is_hdr()) {
                Some(Err(ExportError::Unsupported(format!("VideoToolbox HDR: {why}"))))
            } else {
                None
            };
        }
    };
    let session = if s.dolby_vision { VtEncoder::with_dolby_vision(config, s.ambient_viewing_environment) } else { VtEncoder::with_signal(config, s.signal) };
    match session {
        Ok(vt) => {
            filmcraft_export::note_hw_encode_session();
            let ambient_viewing_environment = vt.ambient_viewing_environment();
            Some(Ok(Box::new(HardwareEncoder {
                vt,
                signal: s.signal,
                dolby_vision: s.dolby_vision,
                ambient_viewing_environment,
                y: Vec::new(),
                u: Vec::new(),
                v: Vec::new(),
                y16: Vec::new(),
                u16: Vec::new(),
                v16: Vec::new(),
                decoded: 0,
            })))
        }
        Err(why) => {
            log::info!("hardware {name} encoding declined: {why}");
            filmcraft_export::note_hw_encode_declined();
            if s.dolby_vision || (format == Format::Hevc && s.signal.is_hdr()) {
                Some(Err(ExportError::Encode(format!("VideoToolbox HDR: {why}"))))
            } else {
                None
            }
        }
    }
}

/// The `hvcC` configuration of an HEVC stream: VideoToolbox's own record when it wrote one.
fn hevc_config(p: &ParamSets) -> Result<HevcConfig, String> {
    let record = p.hvcc.as_deref().ok_or("VideoToolbox wrote no hvcC record for the stream")?;
    let cfg = HevcConfig::parse(record).map_err(|e| format!("unreadable hvcC record: {e}"))?;
    if cfg.vps().is_empty() || cfg.sps().is_empty() || cfg.pps().is_empty() || cfg.length_size != 4 {
        return Err("the hvcC record lacks a parameter set or does not use 4-byte NAL lengths".into());
    }
    Ok(cfg)
}

/// [`VtEncoder`] as a `VideoEncoder`: RGBA pictures in, MP4 samples out.
pub struct HardwareEncoder {
    vt: VtEncoder,
    signal: ColorSignal,
    dolby_vision: bool,
    ambient_viewing_environment: Option<[u8; 8]>,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    y16: Vec<u16>,
    u16: Vec<u16>,
    v16: Vec<u16>,
    /// Compressed frames returned so far.
    decoded: i64,
}

impl HardwareEncoder {
    /// MP4 samples from compressed frames. Without frame reordering every frame is presented at
    /// its decode time, so composition offsets are 0; a frame whose time is not `n × duration`
    /// after the first (a dropped or reordered frame) is an error, never a file with wrong times.
    fn samples(&mut self, packets: Vec<VtPacket>) -> filmcraft_export::Result<Vec<EncodedPacket>> {
        let duration = self.vt.config().frame_duration;
        let mut out = Vec::with_capacity(packets.len());
        for p in packets {
            let n = self.decoded;
            self.decoded += 1;
            if n.checked_mul(i64::from(duration)) != Some(p.pts) || p.pts != p.dts {
                return Err(ExportError::Encode(format!(
                    "VideoToolbox returned frame {n} at time {} (decode time {}), expected {}",
                    p.pts,
                    p.dts,
                    n.saturating_mul(i64::from(duration))
                )));
            }
            out.push(EncodedPacket { data: p.data, key: p.key, duration, composition_offset: 0 });
        }
        Ok(out)
    }
}

impl VideoEncoder for HardwareEncoder {
    fn sample_entry(&self) -> SampleEntry {
        let c = self.vt.config();
        let (w, h) = (c.width.min(u32::from(u16::MAX)) as u16, c.height.min(u32::from(u16::MAX)) as u16);
        match (c.profile.codec(), self.vt.parameter_sets()) {
            (VtCodec::H264, p) => {
                let (sps, pps) = p.map(|p| (p.sps, p.pps)).unwrap_or_default();
                SampleEntry::avc(AvcConfig::new(sps, pps, 4), w, h)
            }
            // `encode` refuses to go on without a usable record, so a stream that got this far has one
            (VtCodec::Hevc, p) => {
                let mut entry = SampleEntry::hevc(p.as_ref().and_then(|p| hevc_config(p).ok()).unwrap_or_default(), w, h);
                self.signal.apply_to(&mut entry, false);
                if self.dolby_vision {
                    if let Some(video) = entry.video.as_mut() {
                        video.dolby_vision =
                            p.as_ref().and_then(|params| params.dolby_vision.clone()).map(|data| DolbyVisionConfig { box_type: FourCc(*b"dvvC"), data });
                        video.ambient_viewing_environment = self.ambient_viewing_environment;
                    }
                }
                entry
            }
        }
    }

    fn timescale(&self) -> u32 {
        self.vt.config().timescale
    }

    fn encode(&mut self, f: &EncoderFrame) -> filmcraft_export::Result<Vec<EncodedPacket>> {
        let c = self.vt.config();
        let codec = c.profile.codec();
        let ten_bit = c.profile == VtProfile::HevcMain10;
        if (f.width, f.height) != (c.width, c.height) {
            return Err(ExportError::Encode(format!("picture is {}x{}, the encoder was created for {}x{}", f.width, f.height, c.width, c.height)));
        }
        let (w, h) = (f.width as usize, f.height as usize);
        let packets = match (ten_bit, f.hdr) {
            (true, Some(rgb)) => {
                let (kr, kb) = self.signal.kr_kb();
                filmcraft_export::timed(filmcraft_export::Stage::Convert, || {
                    filmcraft_export::rgbf_to_yuv420_10(rgb, w, h, kr, kb, &mut self.y16, &mut self.u16, &mut self.v16)
                })?;
                self.vt.encode_10(f.index, &self.y16, &self.u16, &self.v16).map_err(ExportError::Encode)?
            }
            (true, None) => return Err(ExportError::Unsupported("VideoToolbox Main 10 needs the HDR picture".into())),
            (false, Some(_)) => return Err(ExportError::Unsupported("8-bit VideoToolbox encoding does not take HDR pictures".into())),
            (false, None) => {
                if f.rgba.len() < w.saturating_mul(h).saturating_mul(4) {
                    return Err(ExportError::Encode("the RGBA picture is smaller than its size".into()));
                }
                filmcraft_export::timed(filmcraft_export::Stage::Convert, || {
                    filmcraft_export::rgba_to_yuv420_8(f.rgba, w, h, &mut self.y, &mut self.u, &mut self.v)
                });
                self.vt.encode(f.index, &self.y, &self.u, &self.v).map_err(ExportError::Encode)?
            }
        };
        filmcraft_export::note_hw_encode_frame();
        // the muxer needs the parameter sets after the first group of pictures
        if f.index == 0 {
            let params = self.vt.parameter_sets().ok_or_else(|| ExportError::Encode("VideoToolbox returned no parameter sets for the first frame".into()))?;
            if codec == VtCodec::Hevc {
                let header = hevc_config(&params).map_err(ExportError::Encode)?;
                if ten_bit && (header.general_profile_idc != 2 || header.bit_depth_luma != 10 || header.bit_depth_chroma != 10 || header.chroma_format_idc != 1)
                {
                    return Err(ExportError::Encode("VideoToolbox did not produce HEVC Main 10 4:2:0 for the HDR picture".into()));
                }
            }
        }
        self.samples(packets)
    }

    fn flush(&mut self) -> filmcraft_export::Result<Vec<EncodedPacket>> {
        let packets = self.vt.flush().map_err(ExportError::Encode)?;
        self.samples(packets)
    }
}
