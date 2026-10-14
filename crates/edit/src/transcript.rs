//! Text-based editing: the sequence transcript and the edits made through it.
//!
//! Clip transcripts (`filmcraft_project::Transcript`) hold words in **media time**. The sequence
//! transcript ([`sequence_words`]) maps them through the audio track items that play them:
//!
//! - audio tracks are read top first (A1, A2…); a word is taken from the first track whose
//!   transcribed clip covers the word's midpoint, so a dialogue clip duplicated on two tracks (or a
//!   stereo pair split over two mono tracks) reads once;
//! - a word belongs to a clip when its midpoint, mapped through the clip's speed, falls inside the
//!   clip; its timeline bounds are clamped to the clip, so a word cut by an edit is shown cut;
//! - disabled clips, reversed clips and frame holds contribute no words (they don't play speech).
//!
//! Text edits turn word ranges into timeline ranges ([`word_range`]) snapped outward to frames, then
//! reuse the ordinary [`crate::extract`] / [`crate::lift`] edits. Pause and filler-word removal
//! ([`find_pauses`], [`find_fillers`]) produce many ranges that [`ripple_delete_ranges`] removes in
//! one pass, right to left. Captions come from [`caption_blocks`].

use std::collections::BTreeMap;
use std::sync::Arc;

use unicode_general_category::{GeneralCategory, get_general_category};

use filmcraft_project::{Caption, ClipId, ItemId, Sequence, TrackId, Transcript};
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick, TimeRange};

use crate::EditCtx;

/// A word of the sequence transcript.
#[derive(Clone, Debug, PartialEq)]
pub struct SeqWord {
    pub text: String,
    /// Sequence time.
    pub start: Tick,
    pub end: Tick,
    /// The track item it is heard through and that item's media.
    pub clip: ClipId,
    pub item: ItemId,
    /// Index of the word in the media item's transcript.
    pub index: usize,
    /// Audio track index (0 = A1).
    pub track: usize,
    pub speaker: Option<String>,
    pub confidence: f32,
}

impl SeqWord {
    pub fn normalized(&self) -> String {
        filmcraft_project::transcript::normalize_word(&self.text)
    }
    pub fn range(&self) -> TimeRange {
        TimeRange::from_bounds(self.start, self.end.max(self.start))
    }
}

/// Transcripts by media item (as in `Project::transcripts`).
pub type Transcripts = BTreeMap<ItemId, Arc<Transcript>>;

fn ticks(t: f64) -> Tick {
    Tick(t.round() as i64)
}

/// The sequence transcript: words of every transcribed clip on the audio tracks, in sequence
/// time order (see the module docs for the rules).
pub fn sequence_words(seq: &Sequence, transcripts: &Transcripts) -> Vec<SeqWord> {
    let mut out: Vec<SeqWord> = Vec::new();
    // timeline ranges already served by a higher track's transcribed clips
    let mut claimed: Vec<TimeRange> = Vec::new();
    for (ti, track) in seq.audio_tracks.iter().enumerate() {
        let mut mine = Vec::new();
        for it in &track.items {
            if !it.enabled || it.reverse || it.frame_hold.is_some() || it.speed <= 0.0 {
                continue;
            }
            let Some(tr) = transcripts.get(&it.item) else { continue };
            mine.push(it.range());
            let speed = it.speed;
            let media_end = it.source_in + ticks(it.duration.0 as f64 * speed);
            let to_tl = |m: Tick| it.start + ticks((m - it.source_in).0 as f64 / speed);
            for wi in tr.words_in(TimeRange::from_bounds(it.source_in, media_end.max(it.source_in))) {
                let w = &tr.words[wi];
                let (a, b) = (to_tl(w.start), to_tl(w.end.max(w.start)));
                let mid = Tick(a.0 + (b.0 - a.0) / 2);
                if mid < it.start || mid >= it.end() || claimed.iter().any(|r| r.contains(mid)) {
                    continue;
                }
                out.push(SeqWord {
                    text: w.text.clone(),
                    start: a.max(it.start),
                    end: b.min(it.end()).max(a.max(it.start)),
                    clip: it.id,
                    item: it.item,
                    index: wi,
                    track: ti,
                    speaker: tr.speaker_name(w),
                    confidence: w.confidence,
                });
            }
        }
        claimed.extend(mine);
    }
    out.sort_by_key(|w| (w.start, w.track));
    out
}

/// Index of the word being spoken at `t` (the last word starting at or before `t` whose end is
/// after `t`).
pub fn word_at(words: &[SeqWord], t: Tick) -> Option<usize> {
    let i = words.partition_point(|w| w.start <= t).checked_sub(1)?;
    (t < words[i].end).then_some(i)
}

/// Paragraphs (Text panel segments): runs of words split where the speaker changes or at a pause
/// of at least `gap`.
pub fn paragraphs(words: &[SeqWord], gap: Tick) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut a = 0;
    for i in 1..=words.len() {
        if i == words.len() || words[i].speaker != words[i - 1].speaker || words[i].start - words[i - 1].end >= gap {
            if a < i {
                out.push(a..i);
            }
            a = i;
        }
    }
    out
}

/// Matches of `query` (one or more words, case and punctuation ignored; the last query word may be
/// a prefix) as word index ranges.
pub fn search(words: &[SeqWord], query: &str) -> Vec<std::ops::Range<usize>> {
    let q: Vec<String> = query.split_whitespace().map(filmcraft_project::transcript::normalize_word).filter(|s| !s.is_empty()).collect();
    if q.is_empty() {
        return Vec::new();
    }
    let norm: Vec<String> = words.iter().map(SeqWord::normalized).collect();
    let mut out = Vec::new();
    for i in 0..norm.len().saturating_sub(q.len() - 1) {
        let ok = q.iter().enumerate().all(|(k, qw)| if k + 1 == q.len() { norm[i + k].starts_with(qw.as_str()) } else { norm[i + k] == *qw });
        if ok {
            out.push(i..i + q.len());
        }
    }
    out
}

/// Timeline range of words `a..=b`, snapped outward to frames.
pub fn word_range(words: &[SeqWord], a: usize, b: usize, rate: FrameRate) -> Option<TimeRange> {
    let (a, b) = (a.min(b), a.max(b));
    let (s, e) = (words.get(a)?.start, words.get(b)?.end);
    let s = rate.snap(s);
    let mut e2 = rate.snap(e);
    if e2 < e || e2 <= s {
        e2 += rate.frame_duration();
    }
    Some(TimeRange::from_bounds(s, e2))
}

/// Media range of words `a..=b` of a clip transcript, snapped outward to the media's frames (for
/// Source-monitor In/Out from a text selection).
pub fn media_word_range(t: &Transcript, a: usize, b: usize, rate: FrameRate) -> Option<TimeRange> {
    let (a, b) = (a.min(b), a.max(b));
    let (s, e) = (t.words.get(a)?.start, t.words.get(b)?.end);
    let s = rate.snap(s);
    let mut e2 = rate.snap(e);
    if e2 < e || e2 <= s {
        e2 += rate.frame_duration();
    }
    Some(TimeRange::from_bounds(s, e2))
}

/// Pauses between consecutive words of at least `min`, as the ranges to remove: each keeps `keep`
/// of silence next to both words and is snapped inward to frames (pauses shorter than one frame
/// after that are skipped).
pub fn find_pauses(words: &[SeqWord], min: Tick, keep: Tick, rate: FrameRate) -> Vec<TimeRange> {
    let mut out = Vec::new();
    for p in words.windows(2) {
        let (a, b) = (p[0].end, p[1].start);
        if b - a < min || b <= a {
            continue;
        }
        let s = a + keep;
        let e = b - keep;
        let mut s2 = rate.snap(s);
        if s2 < s {
            s2 += rate.frame_duration();
        }
        let e2 = rate.snap(e);
        if e2 > s2 {
            out.push(TimeRange::from_bounds(s2, e2));
        }
    }
    out
}

/// The default filler words and phrases (configurable in preferences and per command).
pub const DEFAULT_FILLERS: &[&str] = &["um", "uh", "umm", "uhm", "erm", "er", "ah", "hmm", "mm", "mhm"];

/// Filler words: word index ranges matching one of `fillers` (each a word or a phrase such as
/// "you know"; case and punctuation ignored).
pub fn find_fillers(words: &[SeqWord], fillers: &[String]) -> Vec<std::ops::Range<usize>> {
    let norm: Vec<String> = words.iter().map(SeqWord::normalized).collect();
    let mut phrases: Vec<Vec<String>> = fillers
        .iter()
        .map(|f| f.split_whitespace().map(filmcraft_project::transcript::normalize_word).filter(|s| !s.is_empty()).collect::<Vec<_>>())
        .filter(|p| !p.is_empty())
        .collect();
    // longest phrases first, so "you know" wins over a lone "you"
    phrases.sort_by_key(|p| std::cmp::Reverse(p.len()));
    let mut out = Vec::new();
    let mut i = 0;
    while i < norm.len() {
        let hit = phrases.iter().find(|p| i + p.len() <= norm.len() && p.iter().enumerate().all(|(k, w)| norm[i + k] == *w));
        match hit {
            Some(p) => {
                out.push(i..i + p.len());
                i += p.len();
            }
            None => i += 1,
        }
    }
    out
}

/// Timeline ranges removing the filler words `hits`: each word range snapped to the nearest frames
/// (never into the neighbouring words' frames).
pub fn filler_ranges(words: &[SeqWord], hits: &[std::ops::Range<usize>], rate: FrameRate) -> Vec<TimeRange> {
    let mut out = Vec::new();
    for h in hits {
        let (a, b) = (h.start, h.end - 1);
        let mut s = rate.snap_nearest(words[a].start);
        let mut e = rate.snap_nearest(words[b].end);
        if a > 0 && s < words[a - 1].end {
            s = rate.snap(words[a - 1].end) + rate.frame_duration();
        }
        if let Some(n) = words.get(b + 1)
            && e > n.start
        {
            e = rate.snap(n.start);
        }
        if e > s {
            out.push(TimeRange::from_bounds(s, e));
        }
    }
    out
}

/// Sort and merge ranges (overlapping or touching).
pub fn merge_ranges(mut r: Vec<TimeRange>) -> Vec<TimeRange> {
    r.sort_by_key(|x| x.start);
    let mut out: Vec<TimeRange> = Vec::new();
    for x in r {
        match out.last_mut() {
            Some(l) if x.start <= l.end() => *l = TimeRange::from_bounds(l.start, l.end().max(x.end())),
            _ => out.push(x),
        }
    }
    out
}

/// Ripple-delete every range on every unlocked track (and sync-locked caption tracks), right to
/// left so earlier ranges keep their positions. Returns the total time removed.
pub fn ripple_delete_ranges(seq: &mut Sequence, ranges: Vec<TimeRange>, ctx: &mut EditCtx) -> Tick {
    let tracks: Vec<TrackId> = seq.all_tracks().filter(|t| !t.locked).map(|t| t.id).collect();
    let mut total = Tick::ZERO;
    for r in merge_ranges(ranges).into_iter().rev() {
        if r.duration <= Tick::ZERO {
            continue;
        }
        crate::extract(seq, &tracks, r, ctx);
        total += r.duration;
    }
    total
}

/// Rules for Create Captions from a transcript, with short single-line captions by default.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptionRules {
    /// Maximum characters per line.
    pub max_chars: usize,
    /// Maximum whitespace-separated words per line, including corrected transcript phrases.
    pub max_words_per_line: usize,
    /// Lines per caption (1 = single, 2 = double).
    pub lines: usize,
    /// Minimum caption duration; a short caption is extended into the silence after it, never
    /// over the next one.
    pub min_duration: Tick,
    /// Longest caption.
    pub max_duration: Tick,
    /// Frames left empty between consecutive captions.
    pub gap_frames: i64,
    /// A pause at least this long starts a new caption.
    pub break_pause: Tick,
}

impl Default for CaptionRules {
    fn default() -> Self {
        Self {
            max_chars: 42,
            max_words_per_line: 3,
            lines: 1,
            min_duration: Tick(TICKS_PER_SECOND),
            max_duration: Tick(7 * TICKS_PER_SECOND),
            gap_frames: 0,
            break_pause: Tick(TICKS_PER_SECOND),
        }
    }
}

/// A caption block made from words.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptionBlock {
    pub start: Tick,
    pub end: Tick,
    /// Lines joined by `\n`.
    pub text: String,
    pub speaker: Option<String>,
    /// Original sequence-word indices. Blocks can share an index when a corrected transcript
    /// word contains a phrase that must be split across captions.
    pub words: std::ops::Range<usize>,
}

struct CaptionToken<'a> {
    text: String,
    /// Detected in the original text before removing punctuation from display text.
    sentence_end: bool,
    start: Tick,
    end: Tick,
    source: usize,
    speaker: &'a Option<String>,
}

fn caption_punctuation(c: char) -> bool {
    matches!(
        get_general_category(c),
        GeneralCategory::ClosePunctuation
            | GeneralCategory::ConnectorPunctuation
            | GeneralCategory::DashPunctuation
            | GeneralCategory::FinalPunctuation
            | GeneralCategory::InitialPunctuation
            | GeneralCategory::OpenPunctuation
            | GeneralCategory::OtherPunctuation
    )
}

fn push_caption_part(parts: &mut Vec<(String, bool)>, current: &mut String, sentence_end: bool) {
    if !current.is_empty() {
        parts.push((std::mem::take(current), sentence_end));
    } else if sentence_end && let Some((_, end)) = parts.last_mut() {
        *end = true;
    }
}

/// Normalize only generated captions: keep sentence boundaries before removing punctuation,
/// and use Unicode uppercase (including Russian). The stored transcript is never changed.
/// Corrections and imports may contain phrases; distribute their existing interval over the
/// nonempty display words using exact tick math. Punctuation alone consumes no word quota.
fn caption_tokens(words: &[SeqWord]) -> Vec<CaptionToken<'_>> {
    let mut tokens: Vec<CaptionToken<'_>> = Vec::new();
    for (source, word) in words.iter().enumerate() {
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut previous: Option<char> = None;
        let mut chars = word.text.chars().peekable();
        while let Some(c) = chars.next() {
            // A decimal point is punctuation to remove, but does not finish a thought.
            let decimal_point = c == '.' && previous.is_some_and(|p| p.is_numeric()) && chars.peek().is_some_and(|n| n.is_numeric());
            let sentence_end = matches!(c, '.' | '?' | '!' | '…' | '。' | '！' | '？' | '．' | '؟') && !decimal_point;
            if sentence_end {
                if current.is_empty() && parts.is_empty() {
                    // Some transcribers emit a full stop as a separate transcript entry.
                    if let Some(last) = tokens.last_mut() {
                        last.sentence_end = true;
                    }
                }
                push_caption_part(&mut parts, &mut current, true);
            } else if c.is_whitespace() || caption_punctuation(c) {
                // Separate adjacent words rather than joining them when a comma/dash has no
                // surrounding spaces. Combining marks and non-punctuation symbols survive.
                push_caption_part(&mut parts, &mut current, false);
            } else {
                current.extend(c.to_uppercase());
            }
            previous = Some(c);
        }
        push_caption_part(&mut parts, &mut current, false);
        let count = parts.len();
        if count == 0 {
            continue;
        }
        let start = i128::from(word.start.0);
        let span = (i128::from(word.end.0) - start).max(0);
        let at = |i: usize| Tick((start + span * i as i128 / count as i128).clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64);
        for (i, (text, sentence_end)) in parts.into_iter().enumerate() {
            tokens.push(CaptionToken { text, sentence_end, start: at(i), end: at(i.saturating_add(1)), source, speaker: &word.speaker });
        }
    }
    tokens
}

/// Lay words out as caption blocks: words fill lines of at most `max_chars` (a longer single word
/// gets a line of its own), `max_words_per_line` words, and blocks of `lines` lines; a new block starts at a speaker change,
/// at a pause of `break_pause`, after `max_duration`, or after any sentence-ending punctuation.
/// Word and character limits are maxima, never a reason to join separate sentences.
/// Times are snapped to frames, blocks never overlap and keep
/// `gap_frames` between them.
pub fn caption_blocks(words: &[SeqWord], rules: &CaptionRules, rate: FrameRate) -> Vec<CaptionBlock> {
    let tokens = caption_tokens(words);
    let words = tokens.as_slice();
    let max_chars = rules.max_chars.max(1);
    let max_words = rules.max_words_per_line.max(1);
    let max_lines = rules.lines.clamp(1, 2);
    let mut groups: Vec<std::ops::Range<usize>> = Vec::new();
    let mut a = 0;
    let mut lines: Vec<usize> = vec![0];
    let mut line_words = 0usize;
    for i in 0..words.len() {
        let w = &words[i];
        let len = w.text.chars().count();
        if i > a {
            let prev = &words[i - 1];
            let cur = *lines.last().unwrap_or(&0);
            let fits_line = cur.saturating_add(1).saturating_add(len) <= max_chars && line_words < max_words;
            let fits = fits_line || lines.len() < max_lines;
            let brk =
                !fits || w.speaker != prev.speaker || w.start - prev.end >= rules.break_pause || w.end - words[a].start > rules.max_duration || prev.sentence_end;
            if brk {
                groups.push(a..i);
                a = i;
                lines = vec![len];
                line_words = 1;
                continue;
            }
            if fits_line {
                if let Some(l) = lines.last_mut() {
                    *l = l.saturating_add(1).saturating_add(len);
                }
                line_words = line_words.saturating_add(1);
            } else {
                lines.push(len);
                line_words = 1;
            }
        } else {
            lines = vec![len];
            line_words = 1;
        }
    }
    if a < words.len() {
        groups.push(a..words.len());
    }
    // text, frame-snapped times
    let fd = rate.frame_duration();
    let gap = Tick(fd.0.saturating_mul(rules.gap_frames.max(0)));
    let mut out: Vec<CaptionBlock> = Vec::new();
    for g in groups {
        let mut text_lines: Vec<String> = vec![String::new()];
        let mut line_words = 0usize;
        for w in &words[g.clone()] {
            let Some(l) = text_lines.last_mut() else { break };
            if l.is_empty() {
                l.push_str(&w.text);
                line_words = 1;
            } else if l.chars().count().saturating_add(1).saturating_add(w.text.chars().count()) <= max_chars && line_words < max_words {
                l.push(' ');
                l.push_str(&w.text);
                line_words = line_words.saturating_add(1);
            } else {
                text_lines.push(w.text.to_string());
                line_words = 1;
            }
        }
        let start = rate.snap(words[g.start].start);
        let mut end = rate.snap(words[g.end - 1].end);
        if end < words[g.end - 1].end {
            end += fd;
        }
        out.push(CaptionBlock {
            start,
            end: end.max(start + fd),
            text: text_lines.join("\n"),
            speaker: (*words[g.start].speaker).clone(),
            words: words[g.start].source..words[g.end - 1].source.saturating_add(1),
        });
    }
    // no overlaps, minimum duration (into the silence after a block, never over the next one)
    for i in 0..out.len() {
        if i > 0 {
            let min_start = out[i - 1].end + gap;
            if out[i].start < min_start {
                out[i].start = min_start;
                out[i].end = out[i].end.max(min_start + fd);
            }
        }
        let next = out.get(i + 1).map(|n| n.start);
        let b = &mut out[i];
        if b.end - b.start < rules.min_duration {
            let want = b.start + rules.min_duration;
            let mut e = rate.snap(want);
            if e < want {
                e += fd;
            }
            b.end = e;
        }
        if let Some(n) = next {
            b.end = b.end.min(n - gap).max(b.start + fd);
        }
    }
    out
}

/// Caption blocks as captions (ids from `ctx`).
pub fn blocks_to_captions(blocks: &[CaptionBlock], ctx: &mut EditCtx) -> Vec<Caption> {
    blocks
        .iter()
        .map(|b| Caption {
            id: ClipId(ctx.alloc()),
            start: b.start,
            duration: b.end - b.start,
            text: b.text.clone(),
            speaker: b.speaker.clone(),
            cue_id: None,
            settings: String::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests;
