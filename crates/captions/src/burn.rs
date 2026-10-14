//! Caption layout and burn-in.
//!
//! [`overlay`] lays out the caption showing on a track at a time with the track's
//! [`CaptionStyle`] (size relative to a 1080-line frame, colour, background box, outline,
//! alignment, vertical position and margin; explicit vertical placement overrides WebVTT `line:N%`)
//! and rasterises it into a small premultiplied linear-light RGBA [`Overlay`] positioned in the
//! frame. Renderers composite overlays over the finished picture.

use filmcraft_color::srgb_to_linear;
use filmcraft_project::{Caption, CaptionAlign, CaptionAnchor, CaptionStyle, CaptionTrack, Sequence};
use filmcraft_time::Tick;

use filmcraft_text::{ParagraphStyle, TextStyle, layout, render};

/// A rendered caption: premultiplied linear-light RGBA f32 pixels at `(x, y)` in the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Overlay {
    pub x: i32,
    pub y: i32,
    pub w: usize,
    pub h: usize,
    pub px: Vec<f32>,
}

impl Overlay {
    /// Composite over a premultiplied RGBA f32 canvas of `cw`×`ch` pixels.
    pub fn composite_onto(&self, canvas: &mut [f32], cw: usize, ch: usize) {
        for y in 0..self.h {
            let cy = self.y + y as i32;
            if cy < 0 || cy >= ch as i32 {
                continue;
            }
            for x in 0..self.w {
                let cx = self.x + x as i32;
                if cx < 0 || cx >= cw as i32 {
                    continue;
                }
                let s = &self.px[(y * self.w + x) * 4..(y * self.w + x) * 4 + 4];
                if s[3] <= 0.0 {
                    continue;
                }
                let i = (cy as usize * cw + cx as usize) * 4;
                let k = 1.0 - s[3];
                for c in 0..4 {
                    canvas[i + c] = s[c] + canvas[i + c] * k;
                }
            }
        }
    }
}

/// Use the actual selected face, including Black Italic, instead of forcing SemiBold.
fn caption_style(style: &CaptionStyle, px: f32) -> TextStyle {
    let family = if style.font.trim().is_empty() { filmcraft_text::fonts::DEFAULT_FAMILY } else { style.font.trim() };
    let face = if style.font_style.trim().is_empty() { "SemiBold" } else { style.font_style.trim() };
    TextStyle { family: family.into(), style: face.into(), size: px, ..Default::default() }
}

fn bounded(v: f32, fallback: f32, min: f32, max: f32) -> f32 {
    if v.is_finite() { v.clamp(min, max) } else { fallback }
}

fn lin(c: [u8; 4]) -> [f32; 4] {
    let a = c[3] as f32 / 255.0;
    [srgb_to_linear(c[0] as f32 / 255.0) * a, srgb_to_linear(c[1] as f32 / 255.0) * a, srgb_to_linear(c[2] as f32 / 255.0) * a, a]
}

/// Word-wrap one line to `max_w` pixels.
fn wrap(line: &str, ts: &TextStyle, max_w: f32) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in line.split(' ') {
        let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if !cur.is_empty() && filmcraft_text::measure(&cand, ts) > max_w {
            out.push(std::mem::take(&mut cur));
            cur = word.to_string();
        } else {
            cur = cand;
        }
    }
    out.push(cur);
    out
}

/// Placement overrides from WebVTT cue settings.
fn vtt_overrides(settings: &str) -> (Option<f32>, Option<CaptionAlign>) {
    let mut line = None;
    let mut align = None;
    for kv in settings.split_whitespace() {
        let Some((k, v)) = kv.split_once(':') else { continue };
        match k {
            "line" => {
                let v = v.split(',').next().unwrap_or("");
                if let Some(p) = v.strip_suffix('%').and_then(|p| p.parse::<f32>().ok()) {
                    line = Some((p / 100.0).clamp(0.0, 1.0));
                }
            }
            "align" => {
                align = match v {
                    "start" | "left" => Some(CaptionAlign::Left),
                    "end" | "right" => Some(CaptionAlign::Right),
                    "center" | "middle" => Some(CaptionAlign::Center),
                    _ => None,
                }
            }
            _ => {}
        }
    }
    (line, align)
}

/// Lay out and rasterise one caption for a `w`×`h` frame.
pub fn render_caption(c: &Caption, style: &CaptionStyle, w: usize, h: usize) -> Option<Overlay> {
    render_caption_with_white(c, style, w, h, 1.0)
}

/// Render lettering with an HDR graphics-white gain. Box, outline, shadow and alpha retain
/// their original values; only the linear RGB of the letter fill receives the gain.
fn render_caption_with_white(c: &Caption, style: &CaptionStyle, w: usize, h: usize, white_gain: f32) -> Option<Overlay> {
    if w == 0 || h == 0 || w > 32_768 || h > 32_768 {
        return None;
    }
    let scale = h as f32 / 1080.0;
    let px = (bounded(style.size, 54.0, 4.0, 400.0) * scale).max(4.0);
    let max_w = w as f32 * 0.9;
    let ts = caption_style(style, px);
    let mut lines: Vec<String> = Vec::new();
    // (speaker names are metadata; like Premiere, they are not burned in)
    for l in c.plain_lines() {
        lines.extend(wrap(l.trim(), &ts, max_w).into_iter().take(128usize.saturating_sub(lines.len())));
        if lines.len() >= 128 {
            break;
        }
    }
    lines.retain(|l| !l.is_empty());
    if lines.is_empty() {
        return None;
    }
    let vm = filmcraft_text::fonts::face(filmcraft_text::resolve(&ts.family, &ts.style).face).metrics(px);
    let (asc, desc) = (vm.ascent, vm.descent);
    let lh = px * bounded(style.line_spacing, 1.25, 0.8, 4.0);
    let pad_x = (px * 0.3).round();
    let block_h = lh * lines.len() as f32;
    let (line_override, align_override) = vtt_overrides(&c.settings);
    let align = align_override.unwrap_or(style.align);
    let margin = bounded(style.margin, 0.08, 0.0, 0.45) * h as f32;
    let top = if let Some(position) = style.position_y.filter(|v| v.is_finite()) {
        let max_top = (h as f32 - block_h).max(0.0);
        let safe_margin = margin.min(max_top * 0.5);
        safe_margin + (max_top - 2.0 * safe_margin) * position.clamp(0.0, 100.0) / 100.0
    } else {
        match (line_override, style.anchor) {
            (Some(f), _) => (f * h as f32).min(h as f32 - block_h),
            (None, CaptionAnchor::Top) => margin,
            (None, CaptionAnchor::Middle) => (h as f32 - block_h) / 2.0,
            (None, CaptionAnchor::Bottom) => h as f32 - margin - block_h,
        }
    }
    .max(0.0)
    .round();
    let side = w as f32 * 0.05;
    let widths: Vec<f32> = lines.iter().map(|l| filmcraft_text::measure(l, &ts)).collect();
    let xs: Vec<f32> = widths
        .iter()
        .map(|&lw| match align {
            CaptionAlign::Left => side + pad_x,
            CaptionAlign::Right => w as f32 - side - pad_x - lw,
            CaptionAlign::Center => (w as f32 - lw) / 2.0,
        })
        .collect();
    let outline = (bounded(style.outline, 0.0, 0.0, 40.0) * scale).min(64.0);
    let shadow_offset = if style.shadow {
        style.shadow_offset.map(|v| (bounded(v, 0.0, -40.0, 40.0) * scale).clamp(-96.0, 96.0).round() as i32)
    } else {
        [0, 0]
    };
    let [sx, sy] = shadow_offset;
    let grow = outline.ceil() + 1.0;
    let bx0 = (xs.iter().map(|x| x - pad_x - grow).fold(f32::MAX, f32::min) + sx.min(0) as f32).floor().max(-128.0);
    let bx1 = (xs.iter().zip(&widths).map(|(x, lw)| x + lw + pad_x + grow).fold(f32::MIN, f32::max) + sx.max(0) as f32)
        .ceil().min(w as f32 + 128.0);
    let by0 = (top - grow + sy.min(0) as f32).floor().max(-128.0);
    let by1 = (top + block_h + grow + sy.max(0) as f32).ceil().min(h as f32 + 128.0);
    let (ox, oy) = (bx0 as i32, by0 as i32);
    let ow = (bx1 - bx0).max(1.0) as usize;
    let oh = (by1 - by0).max(1.0) as usize;
    let pixels = ow.checked_mul(oh).filter(|n| *n <= 8 * 1024 * 1024)?;
    let channels = pixels.checked_mul(4)?;
    let mut cover = vec![0.0f32; pixels];
    let mut bg = vec![0.0f32; pixels];
    for (i, line) in lines.iter().enumerate() {
        let lt = top + lh * i as f32;
        if style.background && style.background_color[3] > 0 {
            let (x0, x1) = (xs[i] - pad_x, xs[i] + widths[i] + pad_x);
            let (y0, y1) = (lt, lt + lh);
            for y in (y0.floor() as i32).max(oy)..(y1.ceil() as i32).min(oy + oh as i32) {
                let fy = ((y as f32 + 1.0).min(y1) - (y as f32).max(y0)).clamp(0.0, 1.0);
                for x in (x0.floor() as i32).max(ox)..(x1.ceil() as i32).min(ox + ow as i32) {
                    let fx = ((x as f32 + 1.0).min(x1) - (x as f32).max(x0)).clamp(0.0, 1.0);
                    let j = (y - oy) as usize * ow + (x - ox) as usize;
                    bg[j] = (bg[j] + fx * fy).min(1.0);
                }
            }
        }
        let baseline = lt + (lh - (asc + desc)) / 2.0 + asc;
        let l = layout(line, &ts, &ParagraphStyle::default());
        let mut m = filmcraft_text::Mask { w: ow, h: oh, a: std::mem::take(&mut cover) };
        render::draw(&l, &render::at(xs[i], baseline.round()), &mut m, (ox, oy));
        cover = m.a;
    }
    let stroke = if outline > 0.0 { dilate(&cover, ow, oh, outline) } else { Vec::new() };
    let shadow = if style.shadow && style.shadow_color[3] > 0 && shadow_offset != [0, 0] {
        extrude(if stroke.is_empty() { &cover } else { &stroke }, ow, oh, sx, sy)
    } else {
        Vec::new()
    };
    let (mut tc, bc, oc, sc) = (lin(style.color), lin(style.background_color), lin(style.outline_color), lin(style.shadow_color));
    let white_gain = bounded(white_gain, 1.0, 0.0, 64.0);
    for channel in tc.iter_mut().take(3) {
        *channel *= white_gain;
    }
    let mut out = vec![0.0f32; channels];
    for j in 0..pixels {
        let mut p = [0.0f32; 4];
        let over = |p: &mut [f32; 4], c: [f32; 4], a: f32| {
            if a > 0.0 {
                for k in 0..4 {
                    p[k] = c[k] * a + p[k] * (1.0 - c[3] * a);
                }
            }
        };
        over(&mut p, bc, bg[j]);
        if !shadow.is_empty() {
            over(&mut p, sc, shadow[j]);
        }
        if !stroke.is_empty() {
            over(&mut p, oc, stroke[j]);
        }
        over(&mut p, tc, cover[j]);
        out[j * 4..j * 4 + 4].copy_from_slice(&p);
    }
    Some(Overlay { x: ox, y: oy, w: ow, h: oh, px: out })
}

/// Sweep the glyph silhouette along a short offset to form connected, solid side faces.
/// Callers bound each offset to 96 pixels and the mask to eight million pixels.
fn extrude(mask: &[f32], w: usize, h: usize, dx: i32, dy: i32) -> Vec<f32> {
    let steps = dx.abs().max(dy.abs()).max(1);
    let mut out = vec![0.0f32; mask.len()];
    for step in 1..=steps {
        let ox = (dx as f32 * step as f32 / steps as f32).round() as i32;
        let oy = (dy as f32 * step as f32 / steps as f32).round() as i32;
        for y in 0..h {
            let ty = y as i32 + oy;
            if ty < 0 || ty >= h as i32 {
                continue;
            }
            for x in 0..w {
                let tx = x as i32 + ox;
                if tx >= 0 && tx < w as i32 {
                    let target = ty as usize * w + tx as usize;
                    out[target] = out[target].max(mask[y * w + x]);
                }
            }
        }
    }
    out
}

/// Max-filter a coverage mask with a disc of radius `r`.
fn dilate(m: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
    let ri = r.ceil() as i32;
    let mut offs = Vec::new();
    for dy in -ri..=ri {
        for dx in -ri..=ri {
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            if d <= r + 0.5 {
                offs.push((dx, dy, (r + 0.5 - d).clamp(0.0, 1.0)));
            }
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let mut v = 0.0f32;
            for &(dx, dy, wgt) in &offs {
                let (sx, sy) = (x + dx, y + dy);
                if sx >= 0 && sy >= 0 && sx < w as i32 && sy < h as i32 {
                    v = v.max(m[sy as usize * w + sx as usize] * wgt);
                }
            }
            out[y as usize * w + x as usize] = v;
        }
    }
    out
}

/// The overlay for a caption track at timeline time `t` (None when nothing shows).
pub fn track_overlay(track: &CaptionTrack, t: Tick, w: usize, h: usize) -> Option<Overlay> {
    if !track.enabled {
        return None;
    }
    let c = track.caption_at(t)?;
    render_caption(c, &track.style, w, h)
}

/// Overlays of every visible caption track of a sequence at `t`, bottom track first.
pub fn sequence_overlays(seq: &Sequence, t: Tick, w: usize, h: usize) -> Vec<Overlay> {
    seq.caption_tracks.iter().rev().filter_map(|tr| track_overlay(tr, t, w, h)).collect()
}

/// HDR lettering always uses the mastering peak as graphics white (1000 nits).
/// `max_gain` caps preview to the screen's current EDR headroom; HDR export uses the full peak.
/// Older per-track brightness values are no longer read, so saved projects use the same white.
pub fn sequence_overlays_hdr(seq: &Sequence, t: Tick, w: usize, h: usize, max_gain: f32) -> Vec<Overlay> {
    let max_gain = bounded(max_gain, 1.0, 1.0, 64.0);
    let gain = ((filmcraft_color::transform::HDR_PEAK_NITS / filmcraft_color::REFERENCE_WHITE_NITS) as f32).min(max_gain);
    seq.caption_tracks.iter().rev().filter_map(|tr| {
        if !tr.enabled {
            return None;
        }
        let caption = tr.caption_at(t)?;
        render_caption_with_white(caption, &tr.style, w, h, gain)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_project::{CaptionFormat, ClipId, TrackId};

    fn cap(text: &str) -> Caption {
        Caption { id: ClipId(1), start: Tick(0), duration: Tick(100), text: text.into(), speaker: None, cue_id: None, settings: String::new() }
    }

    #[test]
    fn bottom_centre_with_box() {
        let st = CaptionStyle::default();
        let o = render_caption(&cap("Hello world"), &st, 1920, 1080).unwrap();
        // centred horizontally, in the lower part of the frame
        let cx = o.x as f32 + o.w as f32 / 2.0;
        assert!((cx - 960.0).abs() < 4.0, "centre {cx}");
        assert!(o.y > 800 && (o.y as usize + o.h) < 1080, "y {} h {}", o.y, o.h);
        // white text pixels exist and the box is dark translucent
        let mut white = 0;
        let mut boxy = 0;
        for p in o.px.chunks(4) {
            if p[3] > 0.99 && p[0] > 0.9 {
                white += 1;
            } else if p[3] > 0.7 && p[0] < 0.01 {
                boxy += 1;
            }
        }
        assert!(white > 500, "{white}");
        assert!(boxy > 1000, "{boxy}");
    }

    #[test]
    fn top_anchor_and_vtt_line() {
        let st = CaptionStyle { anchor: CaptionAnchor::Top, ..Default::default() };
        let o = render_caption(&cap("Top"), &st, 1280, 720).unwrap();
        assert!(o.y < 100);
        let mut c = cap("Line");
        c.settings = "line:50% align:start".into();
        let o = render_caption(&c, &CaptionStyle::default(), 1280, 720).unwrap();
        assert!((o.y - 360).abs() < 4, "{}", o.y);
        assert!(o.x < 120, "{}", o.x);
    }

    #[test]
    fn wraps_and_composites() {
        let long = "word ".repeat(60);
        let o = render_caption(&cap(&long), &CaptionStyle::default(), 640, 360).unwrap();
        assert!(o.w <= 640 && o.h > 40, "{}x{}", o.w, o.h);
        let mut canvas = vec![0.0f32; 640 * 360 * 4];
        o.composite_onto(&mut canvas, 640, 360);
        assert!(canvas.chunks(4).any(|p| p[0] > 0.9));
    }

    #[test]
    fn outline_draws_around_text() {
        let st = CaptionStyle { background: false, outline: 4.0, outline_color: [255, 0, 0, 255], ..Default::default() };
        let o = render_caption(&cap("O"), &st, 1920, 1080).unwrap();
        assert!(o.px.chunks(4).any(|p| p[0] > 0.9 && p[1] < 0.05 && p[3] > 0.9), "red outline pixels");
    }

    #[test]
    fn track_font_is_used() {
        // the style's font used to be ignored (always Inter)
        let inter = render_caption(&cap("Hello world"), &CaptionStyle::default(), 1920, 1080).unwrap();
        let serif = CaptionStyle { font: "Noto Serif".into(), ..Default::default() };
        let serif = render_caption(&cap("Hello world"), &serif, 1920, 1080).unwrap();
        assert_ne!((inter.w, &inter.px), (serif.w, &serif.px), "Noto Serif renders like Inter");
        let mono = CaptionStyle { font: "jetbrains mono".into(), ..Default::default() };
        let mono = render_caption(&cap("Hello world"), &mono, 1920, 1080).unwrap();
        assert_ne!((inter.w, &inter.px), (mono.w, &mono.px), "JetBrains Mono renders like Inter");
    }

    #[test]
    fn unknown_or_empty_font_falls_back_to_inter() {
        let inter = render_caption(&cap("Hello"), &CaptionStyle::default(), 1280, 720).unwrap();
        for font in ["", "  ", "No Such Font 123"] {
            let st = CaptionStyle { font: font.into(), ..Default::default() };
            assert_eq!(render_caption(&cap("Hello"), &st, 1280, 720), Some(inter.clone()), "font {font:?}");
        }
    }

    #[test]
    fn hidden_track_draws_nothing() {
        let mut t = CaptionTrack::new(TrackId(1), "S".into(), CaptionFormat::Subtitle);
        t.captions.push(cap("x"));
        assert!(track_overlay(&t, Tick(5), 100, 100).is_some());
        t.enabled = false;
        assert!(track_overlay(&t, Tick(5), 100, 100).is_none());
    }
}
