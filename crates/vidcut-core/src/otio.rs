//! OpenTimelineIO interchange (deviations.md X9): writes the timeline as an
//! `.otio` file and reads one back.
//!
//! **Writing** ([`export`]):
//! - Each video track becomes an OTIO video track of clips and gaps.
//! - The audio of each track's clips becomes an audio track (`V1 audio`, …),
//!   and each audio track (`A1`, …) one more. Each audio clip covers its
//!   clip's audio range, so J/L cuts show up as audio edits offset from the
//!   video edits, which is what other editors expect.
//! - Clips reference their media by `file://` URL. Available ranges start at
//!   0, and all times are frames at the project rate.
//! - Timeline markers go on the top-level stack.
//! - Details OTIO has no field for (clip gain, mute and audio stream, track
//!   settings, marker colors, the In/Out marks) are kept under
//!   `metadata.vidcut`.
//!
//! **Reading** ([`import`]) accepts what Resolve and the OTIO library write:
//! - `Clip.1` and `Clip.2`; source ranges in the media's own timecode
//!   (relative to the reference's available range); float rates, snapped to
//!   exact rationals.
//! - Video tracks become V1, V2, … An audio clip that lines up with a video
//!   clip of the same media (on any video track) becomes that clip's audio,
//!   J/L offsets included. Other audio clips go on audio tracks A1, A2, …
//!   (files vidcut wrote say which track each audio track belongs to).
//! - Everything else is left out with a warning: transitions (cuts instead),
//!   nested compositions and generators (gaps instead).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Marks, Project, TrackKind};
use crate::time::Rational;

/// Where vidcut keeps its own details in OTIO metadata.
const VIDCUT: &str = "vidcut";

fn rt(value: i64, rate: f64) -> Value {
    json!({ "OTIO_SCHEMA": "RationalTime.1", "rate": rate, "value": value as f64 })
}

fn range(start: i64, duration: i64, rate: f64) -> Value {
    json!({
        "OTIO_SCHEMA": "TimeRange.1",
        "start_time": rt(start, rate),
        "duration": rt(duration, rate),
    })
}

fn gap(frames: i64, rate: f64) -> Value {
    json!({
        "OTIO_SCHEMA": "Gap.1",
        "name": "",
        "source_range": range(0, frames, rate),
        "effects": [],
        "markers": [],
        "enabled": true,
        "metadata": {},
    })
}

/// A `file://` URL for an absolute path, with the characters URLs need
/// escaped.
pub fn file_url(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The path of a local `file://` URL, or of a plain path (as some tools
/// write `target_url`).
pub fn url_path(url: &str) -> Option<PathBuf> {
    let rest = match url.strip_prefix("file://") {
        Some(r) if r.starts_with('/') => r,
        Some(r) => r.strip_prefix("localhost").filter(|r| r.starts_with('/'))?,
        None if url.contains("://") => return None,
        None => url,
    };
    let hex = |b: u8| (b as char).to_digit(16);
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok().map(PathBuf::from)
}

/// OTIO's marker color for one of ours.
fn otio_color(c: &str) -> &'static str {
    match c {
        "red" | "rose" => "RED",
        "pink" | "fuchsia" => "PINK",
        "yellow" | "lemon" | "sand" | "cream" => "YELLOW",
        "green" | "mint" => "GREEN",
        "cyan" | "sky" => "CYAN",
        "purple" | "lavender" => "PURPLE",
        "cocoa" => "ORANGE",
        _ => "BLUE",
    }
}

/// Our marker color for one of OTIO's.
fn our_color(c: &str) -> &'static str {
    match c.to_ascii_uppercase().as_str() {
        "RED" => "red",
        "PINK" | "MAGENTA" => "pink",
        "ORANGE" => "cocoa",
        "YELLOW" => "yellow",
        "GREEN" => "green",
        "CYAN" => "cyan",
        "PURPLE" => "purple",
        "WHITE" => "cream",
        _ => "blue",
    }
}

/// The timeline range `[a, b)` of `p` (default: all of it) as an OTIO
/// timeline named `name`.
pub fn export(p: &Project, name: &str, span: Option<(i64, i64)>) -> Result<Value> {
    let rate = p.settings.fps.as_f64();
    let (a, b) = span.unwrap_or((0, p.end()));
    if b <= a {
        return Err(Error::invalid("nothing to export: the range is empty"));
    }
    let item = |label: &str, media: &str, source: i64, frames: i64, enabled, meta: Value| {
        let m = p
            .media(media)
            .ok_or_else(|| Error::internal(format!("no media {media}")))?;
        Ok::<_, Error>(json!({
            "OTIO_SCHEMA": "Clip.2",
            "name": label,
            "source_range": range(source, frames, rate),
            "media_references": {
                "DEFAULT_MEDIA": {
                    "OTIO_SCHEMA": "ExternalReference.1",
                    "name": m.name,
                    "target_url": file_url(&m.path),
                    "available_range": range(0, p.media_frames(m), rate),
                    "available_image_bounds": null,
                    "metadata": {},
                }
            },
            "active_media_reference_key": "DEFAULT_MEDIA",
            "effects": [],
            "markers": [],
            "enabled": enabled,
            "metadata": { VIDCUT: meta },
        }))
    };
    let track = |name: &str, kind: &str, children: Vec<Value>, enabled: bool, meta: &Value| {
        json!({
            "OTIO_SCHEMA": "Track.1",
            "name": name,
            "kind": kind,
            "children": children,
            "source_range": null,
            "effects": [],
            "markers": [],
            "enabled": enabled,
            "metadata": { VIDCUT: meta },
        })
    };
    // A video track for each video track, and an audio track for each track
    // that carries audio; vidcut's track settings go in their metadata.
    let (mut video_tracks, mut audio_tracks) = (Vec::new(), Vec::new());
    for t in &p.tracks {
        let (mut video, mut audio) = (Vec::new(), Vec::new());
        let (mut tv, mut ta) = (a, a);
        for c in &t.clips {
            let label = c
                .name
                .clone()
                .or_else(|| p.media(&c.media).map(|m| m.name.clone()))
                .unwrap_or_default();
            let (s, e) = (c.start.max(a), c.end().min(b));
            if c.video && e > s {
                if s > tv {
                    video.push(gap(s - tv, rate));
                }
                let mut meta = json!({ "clip": c.id, "media": c.media });
                if let Some(au) = &c.audio {
                    meta["stream"] = json!(au.stream);
                    meta["gain_db"] = json!(au.gain_db);
                    meta["muted"] = json!(au.muted);
                }
                video.push(item(
                    &label,
                    &c.media,
                    c.source_at(s),
                    e - s,
                    c.enabled,
                    meta,
                )?);
                tv = e;
            }
            if let (Some(au), Some((ra, rb))) = (&c.audio, c.audio_range()) {
                let (s, e) = (ra.max(a).max(ta), rb.min(b));
                if e > s {
                    if s > ta {
                        audio.push(gap(s - ta, rate));
                    }
                    let meta = json!({ "clip": c.id, "stream": au.stream, "gain_db": au.gain_db });
                    audio.push(item(
                        &label,
                        &c.media,
                        c.source_at(s),
                        e - s,
                        !au.muted,
                        meta,
                    )?);
                    ta = e;
                }
            }
        }
        let meta = json!({
            "track": t.id, "locked": t.locked, "muted": t.muted, "solo": t.solo,
            "volume_db": t.volume_db, "video_enabled": t.video_enabled, "name": t.name,
        });
        if t.kind == TrackKind::Video {
            video_tracks.push(track(&t.id, "Video", video, t.video_enabled, &meta));
            if !audio.is_empty() {
                let name = format!("{} audio", t.id);
                audio_tracks.push(track(&name, "Audio", audio, !t.muted, &meta));
            }
        } else {
            audio_tracks.push(track(&t.id, "Audio", audio, !t.muted, &meta));
        }
    }
    let markers: Vec<Value> = p
        .markers
        .iter()
        .filter(|k| (a..b).contains(&k.at))
        .map(|k| {
            json!({
                "OTIO_SCHEMA": "Marker.2",
                "name": k.name,
                "comment": k.note,
                "color": otio_color(&k.color),
                "marked_range": range(k.at - a, 0, rate),
                "metadata": { VIDCUT: { "color": k.color } },
            })
        })
        .collect();
    let marks = Marks {
        mark_in: p
            .marks
            .mark_in
            .filter(|i| (a..b).contains(i))
            .map(|i| i - a),
        mark_out: p
            .marks
            .mark_out
            .filter(|o| *o > a && *o <= b)
            .map(|o| o - a),
    };
    Ok(json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": name,
        "global_start_time": rt(p.settings.start_tc + a, rate),
        "tracks": {
            "OTIO_SCHEMA": "Stack.1",
            "name": "tracks",
            "children": video_tracks.into_iter().chain(audio_tracks).collect::<Vec<_>>(),
            "source_range": null,
            "effects": [],
            "markers": markers,
            "enabled": true,
            "metadata": {},
        },
        "metadata": { VIDCUT: { "fps": p.settings.fps, "marks": marks } },
    }))
}

/// A clip read from an OTIO file. Frames are at [`Imported::fps`].
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedClip {
    /// The track it goes on: `V<n>` or `A<n>`.
    pub track: String,
    /// The media file, as the OTIO file names it.
    pub path: PathBuf,
    pub name: String,
    pub start: i64,
    pub duration: i64,
    /// Frames from the media's first frame.
    pub source_in: i64,
    /// False for audio-only clips.
    pub video: bool,
    pub enabled: bool,
    /// Audio offsets `(head, tail)` as in `ClipAudio`, if the clip has audio.
    pub audio: Option<(i64, i64)>,
    pub stream: u32,
    pub gain_db: f64,
    pub muted: bool,
}

impl ImportedClip {
    pub fn end(&self) -> i64 {
        self.start + self.duration
    }

    fn audio_range(&self) -> Option<(i64, i64)> {
        self.audio.map(|(h, t)| (self.start + h, self.end() + t))
    }
}

/// A track's settings read from an OTIO file.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedTrack {
    pub id: String,
    pub locked: bool,
    pub muted: bool,
    pub solo: bool,
    pub volume_db: f64,
    pub video_enabled: bool,
    pub name: Option<String>,
}

impl ImportedTrack {
    /// From vidcut's metadata, else from the OTIO track's `enabled`.
    fn of(otio: &Value, id: &str, audio: bool) -> Self {
        let meta = &otio["metadata"][VIDCUT];
        let enabled = otio["enabled"].as_bool().unwrap_or(true);
        Self {
            id: id.to_string(),
            locked: meta["locked"].as_bool().unwrap_or(false),
            muted: meta["muted"].as_bool().unwrap_or(audio && !enabled),
            solo: meta["solo"].as_bool().unwrap_or(false),
            volume_db: meta["volume_db"].as_f64().unwrap_or(0.0),
            video_enabled: meta["video_enabled"].as_bool().unwrap_or(audio || enabled),
            name: meta["name"].as_str().map(String::from),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedMarker {
    pub at: i64,
    pub name: String,
    pub note: String,
    pub color: String,
}

/// A timeline read from an OTIO file.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub name: String,
    pub fps: Rational,
    /// Record timecode of the first frame, in frames.
    pub start_tc: i64,
    /// Sorted by start; on each track, not overlapping.
    pub clips: Vec<ImportedClip>,
    /// Settings of the tracks the clips go on.
    pub tracks: Vec<ImportedTrack>,
    pub markers: Vec<ImportedMarker>,
    pub marks: Marks,
    /// Whether the file has audio tracks. Without any, clips take their
    /// media's audio.
    pub has_audio_tracks: bool,
    pub warnings: Vec<String>,
}

fn schema(v: &Value) -> &str {
    v["OTIO_SCHEMA"].as_str().unwrap_or("")
}

/// `(value, rate)` of a `RationalTime`.
fn time(v: &Value) -> Option<(f64, f64)> {
    let (value, rate) = (v["value"].as_f64()?, v["rate"].as_f64()?);
    (value.is_finite() && rate.is_finite() && rate > 0.0).then_some((value, rate))
}

/// The exact rate for an OTIO rate: whole rates as they are, NTSC rates
/// written exactly (29.97002997…) or rounded (29.97, 23.976), else whatever
/// short fraction it is (12.5 = 25/2).
fn rational(rate: f64) -> Result<Rational> {
    let exact = |den: i64| {
        let num = (rate * den as f64).round();
        (num >= 1.0 && (num / den as f64 - rate).abs() < 1e-6)
            .then(|| Rational::new(num as i64, den).ok())
            .flatten()
    };
    exact(1)
        .or_else(|| exact(1001))
        .or_else(|| Rational::snap_standard(rate))
        .or_else(|| exact(100))
        .or_else(|| exact(1000))
        .ok_or_else(|| Error::usage(format!("unsupported frame rate {rate}")))
}

/// Frames at `fps` for an OTIO time. A rate that is `fps` written as a
/// decimal counts frames as they are, so long timecodes don't drift.
fn frames(value: f64, rate: f64, fps: Rational) -> i64 {
    match rational(rate) {
        Ok(r) if r == fps => value.round() as i64,
        _ => (value * fps.as_f64() / rate).round() as i64,
    }
}

fn find_timeline(v: &Value) -> Option<&Value> {
    if schema(v).starts_with("Timeline.") {
        return Some(v);
    }
    // A SerializableCollection of timelines: the first one.
    v["children"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(find_timeline)
}

fn is_clip(item: &Value) -> bool {
    schema(item).starts_with("Clip.")
}

/// A clip's active media reference (`Clip.1` has one, `Clip.2` several).
fn media_reference(item: &Value) -> &Value {
    if item.get("media_references").is_some() {
        let key = item["active_media_reference_key"]
            .as_str()
            .unwrap_or("DEFAULT_MEDIA");
        &item["media_references"][key]
    } else {
        &item["media_reference"]
    }
}

/// An item's length in frames: its source range, else (for clips) its
/// media's available range, else (for compositions) its children's.
fn item_len(item: &Value, fps: Rational) -> i64 {
    if let Some((v, r)) = time(&item["source_range"]["duration"]) {
        return frames(v, r, fps);
    }
    if is_clip(item) {
        return time(&media_reference(item)["available_range"]["duration"])
            .map_or(0, |(v, r)| frames(v, r, fps));
    }
    let s = schema(item);
    let children = item["children"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| !schema(c).starts_with("Transition"))
        .map(|c| item_len(c, fps));
    if s.starts_with("Track") {
        children.sum()
    } else if s.starts_with("Stack") {
        children.max().unwrap_or(0)
    } else {
        0
    }
}

/// A clip's media path and source start, in frames from the media's first
/// frame. Source ranges count from the available range's start, which
/// Resolve sets to the media's timecode.
fn clip_source(item: &Value, fps: Rational) -> Result<(PathBuf, i64)> {
    let reference = media_reference(item);
    let name = item["name"].as_str().unwrap_or("");
    if !schema(reference).starts_with("ExternalReference") {
        return Err(Error::media(format!(
            "clip {name:?} has no media file ({})",
            match schema(reference) {
                "" => "no reference",
                s => s,
            }
        )));
    }
    let url = reference["target_url"].as_str().unwrap_or("");
    let path = url_path(url)
        .ok_or_else(|| Error::media(format!("clip {name:?}: unsupported media URL {url:?}")))?;
    let available = time(&reference["available_range"]["start_time"]);
    let start = match time(&item["source_range"]["start_time"]) {
        Some((v, r)) => frames(v, r, fps),
        None => available.map_or(0, |(v, r)| frames(v, r, fps)),
    };
    let base = available.map_or(0, |(v, r)| frames(v, r, fps));
    Ok((path, start - base))
}

/// A track's items that take time, as `(start, length, item)`. Transitions
/// overlap their neighbors instead, so they are left out.
fn walk<'a>(
    track: &'a Value,
    fps: Rational,
    warnings: &mut Vec<String>,
) -> Vec<(i64, i64, &'a Value)> {
    let mut out = Vec::new();
    let mut t = 0;
    for item in track["children"].as_array().into_iter().flatten() {
        let s = schema(item);
        if s.starts_with("Transition") {
            warn(warnings, "transitions were left out (cuts instead)");
            continue;
        }
        if s.starts_with("Stack") || s.starts_with("Track") {
            warn(
                warnings,
                "nested stacks and tracks were left out (gaps instead)",
            );
        } else if !s.starts_with("Gap") && !is_clip(item) {
            warn(warnings, &format!("{s} items were left out (gaps instead)"));
        }
        let len = item_len(item, fps);
        out.push((t, len, item));
        t += len;
    }
    out
}

fn warn(warnings: &mut Vec<String>, w: &str) {
    if !warnings.iter().any(|x| x == w) {
        warnings.push(w.to_string());
    }
}

fn overlaps(a: (i64, i64), b: (i64, i64)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// Reads an OTIO document (the parsed JSON of an `.otio` file).
pub fn import(doc: &Value) -> Result<Imported> {
    let tl = find_timeline(doc).ok_or_else(|| Error::usage("no Timeline in this OTIO file"))?;
    let tracks: Vec<&Value> = tl["tracks"]["children"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let of_kind = |kind: &str| -> Vec<&Value> {
        tracks
            .iter()
            .copied()
            .filter(|t| t["kind"] == kind)
            .collect()
    };
    let (video_tracks, audio_tracks) = (of_kind("Video"), of_kind("Audio"));
    let mut warnings = Vec::new();
    // The rate: ours if we wrote the file, else the first clip's, else the
    // timeline's.
    let ours = tl["metadata"][VIDCUT]["fps"]
        .as_str()
        .and_then(|s| s.parse::<Rational>().ok());
    let fps = match ours {
        Some(r) => r,
        None => rational(
            tracks
                .iter()
                .flat_map(|t| t["children"].as_array().into_iter().flatten())
                .filter(|c| is_clip(c))
                .find_map(|c| time(&c["source_range"]["duration"]).map(|(_, r)| r))
                .or_else(|| time(&tl["global_start_time"]).map(|(_, r)| r))
                .ok_or_else(|| Error::usage("this OTIO file has no frame rate"))?,
        )?,
    };
    let start_tc = time(&tl["global_start_time"]).map_or(0, |(v, r)| frames(v, r, fps));

    // Video tracks become V1, V2…
    let mut clips: Vec<ImportedClip> = Vec::new();
    let mut settings: Vec<ImportedTrack> = Vec::new();
    for (k, v) in video_tracks.iter().enumerate() {
        let id = format!("V{}", k + 1);
        settings.push(ImportedTrack::of(v, &id, false));
        for (start, len, item) in walk(v, fps, &mut warnings) {
            if !is_clip(item) || len <= 0 {
                continue;
            }
            match clip_source(item, fps) {
                Ok((path, source_in)) => {
                    let meta = &item["metadata"][VIDCUT];
                    clips.push(ImportedClip {
                        track: id.clone(),
                        path,
                        name: item["name"].as_str().unwrap_or("").to_string(),
                        start,
                        duration: len,
                        source_in,
                        video: true,
                        enabled: item["enabled"].as_bool().unwrap_or(true),
                        audio: None,
                        stream: meta["stream"].as_u64().unwrap_or(0) as u32,
                        gain_db: meta["gain_db"].as_f64().unwrap_or(0.0),
                        muted: meta["muted"].as_bool().unwrap_or(false),
                    });
                }
                Err(e) => warnings.push(format!("{}; left as a gap", e.message)),
            }
        }
    }
    // An audio clip lining up with a video clip of the same media (on the
    // track vidcut named, else on any) is that clip's audio. Others are
    // audio-only clips: on the track vidcut named, else on audio tracks A1,
    // A2… in the file's order.
    let mut foreign_audio = 0;
    for a in &audio_tracks {
        let hint = a["metadata"][VIDCUT]["track"].as_str().map(String::from);
        let mut own: Option<String> = None;
        for (start, len, item) in walk(a, fps, &mut warnings) {
            if !is_clip(item) || len <= 0 {
                continue;
            }
            let (path, source_in) = match clip_source(item, fps) {
                Ok(s) => s,
                Err(e) => {
                    warnings.push(format!("{}; left as a gap", e.message));
                    continue;
                }
            };
            let span = (start, start + len);
            let meta = &item["metadata"][VIDCUT];
            let muted = item["enabled"].as_bool() == Some(false);
            let pairs = |c: &ImportedClip| {
                c.video
                    && c.audio.is_none()
                    && c.path == path
                    && c.source_in - c.start == source_in - start
                    && overlaps((c.start, c.end()), span)
            };
            let owner = match &hint {
                Some(h) if h.starts_with('A') => None,
                Some(h) => clips.iter_mut().find(|c| &c.track == h && pairs(c)),
                None => clips.iter_mut().find(|c| pairs(c)),
            };
            if let Some(c) = owner {
                c.audio = Some((start - c.start, span.1 - c.end()));
                c.muted |= muted;
                if let Some(g) = meta["gain_db"].as_f64() {
                    c.gain_db = g;
                }
                if let Some(s) = meta["stream"].as_u64() {
                    c.stream = s as u32;
                }
                continue;
            }
            let track = match &hint {
                Some(h) => h.clone(),
                None => own
                    .get_or_insert_with(|| {
                        foreign_audio += 1;
                        format!("A{foreign_audio}")
                    })
                    .clone(),
            };
            clips.push(ImportedClip {
                track,
                path,
                name: item["name"].as_str().unwrap_or("").to_string(),
                start,
                duration: len,
                source_in,
                video: false,
                enabled: true,
                audio: Some((0, 0)),
                stream: meta["stream"].as_u64().unwrap_or(0) as u32,
                gain_db: meta["gain_db"].as_f64().unwrap_or(0.0),
                muted,
            });
        }
        match (&hint, own) {
            (Some(h), _) if h.starts_with('A') => settings.push(ImportedTrack::of(a, h, true)),
            (None, Some(id)) => settings.push(ImportedTrack::of(a, &id, true)),
            _ => {}
        }
    }
    // Per track: clips that overlap what is already there lose their audio,
    // or go if that is all they have.
    clips.sort_by(|x, y| x.track.cmp(&y.track).then(x.start.cmp(&y.start)));
    let (mut dropped, mut gone) = (0, 0);
    let mut kept: Vec<ImportedClip> = Vec::new();
    for mut c in clips {
        let same: Vec<&ImportedClip> = kept.iter().filter(|k| k.track == c.track).collect();
        let span = (c.start, c.end());
        let busy = |r: (i64, i64)| {
            same.iter().any(|k| {
                (k.video && overlaps((k.start, k.end()), r))
                    || k.audio_range().is_some_and(|a| overlaps(a, r))
                    || (!k.video && overlaps((k.start, k.end()), r))
            })
        };
        if !c.video && busy(span) {
            gone += 1;
            continue;
        }
        if c.video
            && same
                .iter()
                .any(|k| k.video && overlaps((k.start, k.end()), span))
        {
            gone += 1;
            continue;
        }
        if let Some(r) = c.audio_range()
            && same
                .iter()
                .any(|k| k.audio_range().is_some_and(|a| overlaps(a, r)))
        {
            dropped += 1;
            c.audio = None;
        }
        kept.push(c);
    }
    let mut clips = kept;
    clips.sort_by_key(|c| c.start);
    if dropped > 0 {
        warnings.push(format!(
            "{dropped} clip(s) lost audio that overlapped other audio on their track"
        ));
    }
    if gone > 0 {
        warnings.push(format!(
            "{gone} clip(s) that overlapped others on their track were left out"
        ));
    }
    settings.retain(|t| t.id.starts_with('V') || clips.iter().any(|c| c.track == t.id));

    let mut markers = Vec::new();
    let mut take_markers = |list: &Value| {
        for k in list.as_array().into_iter().flatten() {
            let Some((v, r)) = time(&k["marked_range"]["start_time"]) else {
                continue;
            };
            let color = match k["metadata"][VIDCUT]["color"].as_str() {
                Some(c) if crate::ops::MARKER_COLORS.contains(&c) => c.to_string(),
                _ => our_color(k["color"].as_str().unwrap_or("")).to_string(),
            };
            markers.push(ImportedMarker {
                at: frames(v, r, fps),
                name: k["name"].as_str().unwrap_or("").to_string(),
                note: k["comment"].as_str().unwrap_or("").to_string(),
                color,
            });
        }
    };
    take_markers(&tl["tracks"]["markers"]);
    if let Some(v) = video_tracks.first() {
        take_markers(&v["markers"]);
    }
    markers.retain(|k| k.at >= 0);
    markers.sort_by_key(|k| k.at);
    let marks: Marks =
        serde_json::from_value(tl["metadata"][VIDCUT]["marks"].clone()).unwrap_or_default();
    Ok(Imported {
        name: tl["name"].as_str().unwrap_or("").to_string(),
        fps,
        start_tc,
        clips,
        tracks: settings,
        markers,
        marks,
        has_audio_tracks: !audio_tracks.is_empty(),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AudioInfo, MediaInfo};
    use crate::ops::MAIN;
    use crate::test_support::{FakeEnv, Fixture, media_info};

    fn req(method: &str, params: Value) -> Value {
        json!({ "method": method, "params": params })
    }

    /// Every track with everything an OTIO file can carry, and the media
    /// path of each clip.
    fn clips(p: &Project) -> Vec<(String, crate::model::Clip, PathBuf)> {
        p.tracks
            .iter()
            .flat_map(|t| {
                t.clips.iter().map(|c| {
                    (
                        t.id.clone(),
                        c.clone(),
                        p.media(&c.media).unwrap().path.clone(),
                    )
                })
            })
            .collect()
    }

    /// A timeline with J and L cuts, a gap, an audio-only clip, a disabled,
    /// a muted and a renamed clip, gain, markers and marks.
    fn busy() -> Fixture {
        let mut f = Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)]);
        f.append("m1", 0, 100); // c1 0..100
        f.append("m2", 10, 60); // c2 100..150
        f.append("m1", 100, 150); // c3 150..200
        f.append("m3", 0, 40); // c4 200..240, no audio
        f.append("m2", 0, 50); // c5 240..290
        let ok = |f: &mut Fixture, m: &str, p: Value| {
            f.ok(req(m, p));
        };
        // L cut c1|c2, J cut c2|c3.
        ok(
            &mut f,
            "edit.audio_roll",
            json!({"clip": "c1", "edge": "tail", "by": 5}),
        );
        ok(
            &mut f,
            "edit.audio_roll",
            json!({"clip": "c3", "edge": "head", "by": -7}),
        );
        ok(
            &mut f,
            "clip.set",
            json!({"clips": "c2", "gain_db": -6.0, "name": "interview"}),
        );
        ok(&mut f, "clip.set", json!({"clips": "c3", "enabled": false}));
        ok(&mut f, "clip.set", json!({"clips": "c5", "muted": true}));
        // A gap where c4 was, then audio only in part of it.
        ok(&mut f, "edit.delete", json!({"clips": "c4", "lift": true}));
        ok(
            &mut f,
            "edit.overwrite",
            json!({"media": "m2", "range": "0f..20f", "at": 210, "use_marks": false}),
        );
        ok(
            &mut f,
            "markers.add",
            json!({"at": 120, "name": "look", "color": "mint", "note": "check"}),
        );
        ok(&mut f, "markers.add", json!({"at": 0, "color": "red"}));
        ok(&mut f, "marks.set", json!({"in": 30, "out": 260}));
        f
    }

    #[test]
    fn round_trip_is_frame_exact() {
        let mut f = busy();
        // An audio-only clip in the gap (Audio Only edits make these).
        let p = f.editor.project().clone();
        let gapped = p.clip("c6").unwrap().clone();
        assert_eq!((gapped.start, gapped.duration), (210, 20));
        let mut audio_only = p.clone();
        audio_only
            .track_mut(MAIN)
            .unwrap()
            .clips
            .iter_mut()
            .for_each(|c| {
                if c.id == "c6" {
                    c.video = false;
                }
            });
        f.editor = crate::Editor::new(audio_only, Default::default());
        let before = f.editor.project().clone();
        assert!(crate::validate::violations(&before).is_empty());

        // Through text, as a file would be.
        let text = serde_json::to_string_pretty(&export(&before, "busy", None).unwrap()).unwrap();
        let doc: Value = serde_json::from_str(&text).unwrap();
        let mut g = Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)]);
        let r = g.ok(req("timeline.import", json!({ "otio": doc })));
        assert_eq!(r.result["warnings"], json!([]), "{}", r.result);
        let after = g.editor.project();
        assert_eq!(after.settings.fps, before.settings.fps);
        let strip = |v: Vec<(String, crate::model::Clip, PathBuf)>| -> Vec<_> {
            v.into_iter()
                .map(|(t, mut c, path)| {
                    c.id.clear();
                    (t, c, path)
                })
                .collect()
        };
        assert_eq!(strip(clips(after)), strip(clips(&before)));
        let marker = |p: &Project| -> Vec<_> {
            p.markers
                .iter()
                .map(|k| (k.at, k.name.clone(), k.color.clone(), k.note.clone()))
                .collect()
        };
        assert_eq!(marker(after), marker(&before));
        assert_eq!(after.marks, before.marks);

        // And the same file again appends a second copy after the end.
        let end = after.end();
        let r = g.ok(req("timeline.import", json!({ "otio": doc })));
        assert_eq!(r.result["clips"].as_array().unwrap().len(), 5);
        let p = g.editor.project();
        assert_eq!(p.track(MAIN).unwrap().clips[5].start, end);
        assert_eq!(p.markers.len(), 4);
        assert_eq!(p.media.len(), 3, "media already in the pool are reused");
        // With `replace` it is one copy again.
        g.ok(req(
            "timeline.import",
            json!({ "otio": doc, "replace": true }),
        ));
        assert_eq!(strip(clips(g.editor.project())), strip(clips(&before)));
        // One undo step each.
        g.ok(req("history.undo", json!({})));
        assert_eq!(g.editor.project().track(MAIN).unwrap().clips.len(), 10);
    }

    #[test]
    fn ranges_and_rate_conversion() {
        let f = busy();
        let p = f.editor.project();
        // 100..200 holds c2 (with c1's L-cut audio before it) and c3.
        let doc = export(p, "part", Some((100, 200))).unwrap();
        let t = import(&doc).unwrap();
        assert_eq!(
            t.clips
                .iter()
                .map(|c| (c.start, c.duration, c.source_in, c.audio))
                .collect::<Vec<_>>(),
            vec![(0, 50, 10, Some((5, -7))), (50, 50, 100, Some((-7, 0)))]
        );
        assert_eq!(t.start_tc, 100);
        assert_eq!(t.markers.len(), 1);
        assert_eq!(
            t.marks,
            Marks {
                mark_in: None,
                mark_out: None
            }
        );

        // Into a 50 fps project: positions double, with a warning.
        let mut g = Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)]);
        g.ok(req("project.set", json!({"fps": "50"})));
        g.append("m3", 0, 10);
        let r = g.ok(req("timeline.import", json!({ "otio": doc })));
        // c1's L-cut audio (100..105) came along without c1's video, over c2.
        let w = r.result["warnings"].to_string();
        assert!(
            w.contains("1 clip(s) that overlapped") && w.contains("converted"),
            "{w}"
        );
        let v1 = g.v1();
        assert_eq!(v1[1].1..v1[1].1 + v1[1].2, 10..110);
        assert_eq!(v1[1].3, 20);
    }

    #[test]
    fn missing_media_is_an_error_unless_found_by_name() {
        let f = busy();
        let doc = export(f.editor.project(), "busy", None).unwrap();
        let text = doc.to_string().replace("file:///media/", "file:///gone/");
        let doc: Value = serde_json::from_str(&text).unwrap();
        let mut g = Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)]);
        let e = g.err(req("timeline.import", json!({ "otio": doc })));
        assert_eq!(e.code, crate::ErrorCode::Media);
        assert!(e.message.contains("/gone/1.mp4"), "{}", e.message);
        g.ok(req(
            "timeline.import",
            // Subfolders are searched too.
            json!({ "otio": doc, "search": ["/"] }),
        ));
        assert_eq!(g.v1().len(), 5);
        let e = g.err(req("timeline.import", json!({})));
        assert_eq!(e.code, crate::ErrorCode::Usage);
    }

    /// A file as Resolve writes it: `Clip.1`, source ranges in the media's
    /// timecode relative to its available range, float rates, a transition,
    /// music on the audio track.
    fn resolve_style() -> Value {
        let t = |v: f64| json!({"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": v});
        let r = |s: f64, d: f64| json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": t(s), "duration": t(d)});
        let reference = |file: &str, available: Option<Value>| {
            let mut v = json!({"OTIO_SCHEMA": "ExternalReference.1", "target_url": format!("file:///media/{file}")});
            if let Some(a) = available {
                v["available_range"] = a;
            }
            v
        };
        let clip = |name: &str, range: Value, reference: Value| json!({"OTIO_SCHEMA": "Clip.1", "name": name, "source_range": range, "media_reference": reference});
        let mut a002 = clip("A002", r(10.0, 24.0), reference("A002.mov", None));
        a002["enabled"] = json!(false);
        json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": "Cut",
            "global_start_time": t(86400.0),
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "children": [
                    {"OTIO_SCHEMA": "Track.1", "kind": "Video", "name": "Video 1", "children": [
                        clip("A001", r(86448.0, 48.0), reference("A001.mov", Some(r(86400.0, 1000.0)))),
                        {"OTIO_SCHEMA": "Gap.1", "source_range": r(0.0, 12.0)},
                        {"OTIO_SCHEMA": "Transition.1", "name": "Cross Dissolve",
                         "in_offset": t(6.0), "out_offset": t(6.0)},
                        a002,
                        {"OTIO_SCHEMA": "Clip.1", "name": "Title", "source_range": r(0.0, 24.0),
                         "media_reference": {"OTIO_SCHEMA": "GeneratorReference.1"}}
                    ]},
                    {"OTIO_SCHEMA": "Track.1", "kind": "Video", "name": "Video 2", "children": []},
                    {"OTIO_SCHEMA": "Track.1", "kind": "Audio", "name": "Audio 1", "children": [
                        clip("A001", r(86448.0, 54.0), reference("A001.mov", Some(r(86400.0, 1000.0)))),
                        clip("Music", r(0.0, 30.0), reference("music.wav", None))
                    ]}
                ],
                "markers": [{"OTIO_SCHEMA": "Marker.2", "name": "look", "color": "RED", "comment": "fix",
                    "marked_range": r(30.0, 0.0)}]
            }
        })
    }

    #[test]
    fn reads_resolve_style_files() {
        let t = import(&resolve_style()).unwrap();
        assert_eq!(t.fps, Rational::new(24, 1).unwrap());
        assert_eq!(t.start_tc, 86400);
        let v1: Vec<&ImportedClip> = t.clips.iter().filter(|c| c.track == "V1").collect();
        let a = v1[0];
        assert_eq!((a.start, a.duration, a.source_in), (0, 48, 48));
        // The audio runs 6 frames past the video: an L cut.
        assert_eq!(a.audio, Some((0, 6)));
        let b = v1[1];
        assert_eq!(
            (b.start, b.duration, b.source_in, b.enabled),
            (60, 24, 10, false)
        );
        assert_eq!(b.audio, None);
        assert_eq!(v1.len(), 2, "the title has no media file");
        // The music, alone on its audio track, gets an audio track (A1).
        let music: Vec<&ImportedClip> = t.clips.iter().filter(|c| c.track == "A1").collect();
        assert_eq!(
            (music[0].start, music[0].duration, music[0].video),
            (54, 30, false)
        );
        assert_eq!(
            t.markers,
            vec![ImportedMarker {
                at: 30,
                name: "look".into(),
                note: "fix".into(),
                color: "red".into()
            }]
        );
        let w = t.warnings.join("\n");
        for expected in ["transitions", "GeneratorReference"] {
            assert!(w.contains(expected), "{expected:?} in {w}");
        }
        assert!(!w.contains("overlapped"), "nothing else was left out: {w}");

        // Through the editor: an empty 25 fps project becomes 24 fps from 01:00:00:00.
        let fps = Rational::new(24, 1).unwrap();
        let mut music = media_info(60.0, fps, true);
        music.video = None;
        let mut f = Fixture::new(&[]);
        f.env = FakeEnv::default()
            .with("/media/A001.mov", media_info(1000.0 / 24.0, fps, true))
            .with("/media/A002.mov", media_info(10.0, fps, false))
            .with("/media/music.wav", music);
        let r = f.ok(req("timeline.import", json!({ "otio": resolve_style() })));
        let p = f.editor.project();
        assert_eq!((p.settings.fps, p.settings.start_tc), (fps, 86400));
        assert_eq!(
            f.v1(),
            vec![
                crate::test_support::row("c1", 0, 48, 48),
                crate::test_support::row("c3", 60, 24, 10),
            ]
        );
        assert_eq!(
            f.track("A1"),
            vec![crate::test_support::row("c2", 54, 30, 0)]
        );
        assert_eq!(f.track_ids(), ["V1", "V2", "A1"], "Video 2 comes too");
        let c1 = p.clip("c1").unwrap();
        assert_eq!(c1.audio_range(), Some((0, 54)));
        assert!(p.clip("c3").unwrap().audio.is_none() && !p.clip("c3").unwrap().enabled);
        assert_eq!(r.result["media"], json!(["m1", "m2", "m3"]));
        assert_eq!(p.markers[0].color, "red");
    }

    #[test]
    fn files_without_audio_tracks_bring_media_audio() {
        let mut doc = resolve_style();
        let tracks = doc["tracks"]["children"].as_array_mut().unwrap();
        tracks.retain(|t| t["kind"] == "Video");
        let fps = Rational::new(24, 1).unwrap();
        let mut f = Fixture::new(&[]);
        f.env = FakeEnv::default()
            .with("/media/A001.mov", media_info(1000.0 / 24.0, fps, true))
            .with("/media/A002.mov", media_info(10.0, fps, false));
        f.ok(req("timeline.import", json!({ "otio": doc })));
        let p = f.editor.project();
        assert_eq!(p.clip("c1").unwrap().audio_range(), Some((0, 48)));
        assert!(p.clip("c2").unwrap().audio.is_none(), "A002 has no audio");
        let _ = (AudioInfo::clone, MediaInfo::clone);
    }

    #[test]
    fn urls_round_trip() {
        let p = Path::new("/home/me/My Clips/take #1 (ä).mp4");
        let url = file_url(p);
        assert_eq!(
            url,
            "file:///home/me/My%20Clips/take%20%231%20%28%C3%A4%29.mp4"
        );
        assert_eq!(url_path(&url).unwrap(), p);
        assert_eq!(
            url_path("file://localhost/a/b.mov").unwrap(),
            Path::new("/a/b.mov")
        );
        assert_eq!(
            url_path("/plain/path.mxf").unwrap(),
            Path::new("/plain/path.mxf")
        );
        assert_eq!(url_path("/50%.mov").unwrap(), Path::new("/50%.mov"));
        assert_eq!(url_path("file://server/share/a.mov"), None);
        assert_eq!(url_path("https://example.com/a.mp4"), None);
    }

    #[test]
    fn rates_become_exact() {
        let r = |n, d| Rational::new(n, d).unwrap();
        assert_eq!(rational(24.0).unwrap(), r(24, 1));
        assert_eq!(rational(29.97002997002997).unwrap(), r(30000, 1001));
        assert_eq!(rational(29.97).unwrap(), r(30000, 1001));
        assert_eq!(rational(23.976).unwrap(), r(24000, 1001));
        assert_eq!(rational(59.94).unwrap(), r(60000, 1001));
        assert_eq!(rational(25.0).unwrap(), r(25, 1));
        assert_eq!(rational(12.5).unwrap(), r(25, 2));
        // Ten hours of 23.976 timecode doesn't drift.
        assert_eq!(frames(863_136.0, 23.976, r(24000, 1001)), 863_136);
    }

    #[test]
    fn several_tracks_round_trip() {
        // M6: B-roll on V2, music on A1, track settings, an audio-only clip on
        // V2 and a muted, soloed, faded track.
        let mut f = busy();
        let music = f.add_music("music.wav", 20.0);
        let ok = |f: &mut Fixture, m: &str, p: Value| {
            f.ok(req(m, p));
        };
        ok(&mut f, "marks.clear", json!({}));
        ok(
            &mut f,
            "edit.overwrite",
            json!({"media": "m3", "range": "0f..30f", "at": 20, "track": "V2"}),
        );
        ok(
            &mut f,
            "edit.overwrite",
            json!({"media": "m1", "range": "0f..15f", "at": 60, "track": "V2", "audio_only": true}),
        );
        ok(
            &mut f,
            "edit.overwrite",
            json!({"media": music, "range": "0f..200f", "at": 10}),
        );
        ok(
            &mut f,
            "edit.overwrite",
            json!({"media": music, "range": "300f..350f", "at": 230, "track": "A2"}),
        );
        ok(
            &mut f,
            "track.set",
            json!({"track": "A1", "volume_db": -9.0, "name": "Music"}),
        );
        ok(
            &mut f,
            "track.set",
            json!({"track": "A2", "muted": true, "solo": true}),
        );
        ok(
            &mut f,
            "track.set",
            json!({"track": "V2", "video_enabled": false}),
        );
        let before = f.editor.project().clone();
        let doc = export(&before, "busy", None).unwrap();
        let names: Vec<&str> = doc["tracks"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["V1", "V2", "V1 audio", "V2 audio", "A1", "A2"]);

        let mut g = Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)]);
        g.add_music("music.wav", 20.0);
        let r = g.ok(req("timeline.import", json!({ "otio": doc })));
        assert_eq!(r.result["warnings"], json!([]), "{}", r.result);
        let after = g.editor.project();
        let strip = |v: Vec<(String, crate::model::Clip, PathBuf)>| -> Vec<_> {
            v.into_iter()
                .map(|(t, mut c, path)| {
                    c.id.clear();
                    (t, c, path)
                })
                .collect()
        };
        assert_eq!(strip(clips(after)), strip(clips(&before)));
        let settings = |p: &Project| -> Vec<_> {
            p.tracks
                .iter()
                .map(|t| {
                    (
                        t.id.clone(),
                        t.muted,
                        t.solo,
                        t.volume_db,
                        t.video_enabled,
                        t.name.clone(),
                    )
                })
                .collect()
        };
        assert_eq!(settings(after), settings(&before));
    }

    #[test]
    fn resolve_style_tracks_map_onto_ours() {
        // A Resolve-style file: B-roll with linked audio on Video 2/Audio 2,
        // and music alone on Audio 3.
        let doc = resolve_style();
        let mut doc = doc;
        let tracks = doc["tracks"]["children"].as_array_mut().unwrap();
        // Video 2 gets a clip of A002 over A001, with its audio on a new
        // Audio 2 track; Audio 1's Music clip moves to Audio 3.
        let t = |v: f64| json!({"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": v});
        let r = |s: f64, d: f64| json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": t(s), "duration": t(d)});
        let a002 = json!({"OTIO_SCHEMA": "Clip.1", "name": "A002", "source_range": r(0.0, 12.0),
            "media_reference": {"OTIO_SCHEMA": "ExternalReference.1", "target_url": "file:///media/A002.mov"}});
        let gap = json!({"OTIO_SCHEMA": "Gap.1", "source_range": r(0.0, 10.0)});
        tracks[1]["children"] = json!([gap.clone(), a002.clone()]);
        let music = tracks[2]["children"].as_array_mut().unwrap().pop().unwrap();
        tracks.push(json!({"OTIO_SCHEMA": "Track.1", "kind": "Audio", "name": "Audio 2", "children": [gap, a002]}));
        tracks.push(
            json!({"OTIO_SCHEMA": "Track.1", "kind": "Audio", "name": "Audio 3", "enabled": false,
            "children": [{"OTIO_SCHEMA": "Gap.1", "source_range": r(0.0, 54.0)}, music]}),
        );
        let t = import(&doc).unwrap();
        // (start, duration, video, audio head/tail) of the clips on a track.
        type Placed = (i64, i64, bool, Option<(i64, i64)>);
        let on = |id: &str| -> Vec<Placed> {
            t.clips
                .iter()
                .filter(|c| c.track == id)
                .map(|c| (c.start, c.duration, c.video, c.audio))
                .collect()
        };
        assert_eq!(
            on("V2"),
            vec![(10, 12, true, Some((0, 0)))],
            "linked audio paired across tracks"
        );
        assert_eq!(
            on("A1"),
            vec![(54, 30, false, Some((0, 0)))],
            "music on its own track"
        );
        assert!(
            t.tracks.iter().any(|s| s.id == "A1" && s.muted),
            "disabled track: muted"
        );
        assert!(
            !t.warnings.iter().any(|w| w.contains("audio clip")),
            "{:?}",
            t.warnings
        );
    }
}
