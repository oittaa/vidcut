//! OpenTimelineIO interchange (deviations.md X9): writes the timeline as
//! OTIO and reads OTIO back, in the shapes other editors write and read.
//!
//! **Writing** ([`export`]):
//! - Each video track becomes an OTIO video track of clips and gaps.
//! - The audio of each track's clips becomes an audio track (`V1 audio`, …),
//!   and each audio track (`A1`, …) one more. Each audio clip covers its
//!   clip's audio range, so J/L cuts show up as audio edits offset from the
//!   video edits, which is what other editors expect.
//! - Media are named by plain absolute path, as editors write them (not
//!   every editor reads `file://` URLs), or in a
//!   bundle by a path relative to it. Image sequences are
//!   `ImageSequenceReference`s.
//! - Times are frames at the project rate. Source ranges count from the
//!   media's own start timecode, as their available ranges do: editors
//!   place sources by timecode. A still's available range is one frame. An image sequence's available range counts its
//!   pictures at the sequence's own rate, as OpenTimelineIO counts them.
//! - Editor metadata besides ([`EDITOR`]): a link group joining each clip's
//!   video and audio, the stream and channels its audio plays, clip gain as
//!   a clip volume, track lock and solo, marker notes.
//! - Timeline markers go on the top-level stack, one frame long.
//! - Details OTIO has no field for (clip gain, mute and audio stream, track
//!   settings, marker colors, the In/Out marks) are kept under
//!   `metadata.vidcut`.
//!
//! **Reading** ([`import`]) accepts what other editors and the OTIO library
//! write:
//! - `Clip.1` and `Clip.2`; source ranges in the media's own timecode
//!   (relative to the reference's available range); float rates, snapped to
//!   exact rationals.
//! - Media as `file://` URLs (percent-encoded or not), plain paths, Windows
//!   paths, paths relative to the file, image sequences, and media the
//!   editor had offline (`MissingReference` with a `Media Url`). Each
//!   gives the places to look ([`MediaRef::paths`]), including where a
//!   project folder moved with the timeline file would have put it.
//! - Video tracks become V1, V2, … An audio clip that lines up with a video
//!   clip of the same media (on any video track, in the same link group if
//!   it has one) becomes that clip's audio, J/L offsets included.
//!   Other audio clips go on audio tracks A1, A2, … (files vidcut wrote say
//!   which track each audio track belongs to). The editor metadata's channel
//!   map picks the stream, its clip volume the gain.
//! - Markers on the timeline, on tracks and on clips become timeline
//!   markers.
//! - Everything else is left out with a warning: transitions (cuts instead),
//!   nested compositions and generators (gaps instead), speed changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Marks, Media, Project, TrackKind};
use crate::time::Rational;

#[cfg(test)]
mod tests;

/// Where vidcut keeps its own details in OTIO metadata.
const VIDCUT: &str = "vidcut";
/// Where other editors keep their own details: the key and values are
/// theirs, so that they read what vidcut writes.
const EDITOR: &str = "Resolve_OTIO";

fn rt(value: f64, rate: f64) -> Value {
    json!({ "OTIO_SCHEMA": "RationalTime.1", "rate": rate, "value": value })
}

fn range(start: f64, duration: f64, rate: f64) -> Value {
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
        "source_range": range(0.0, frames as f64, rate),
        "effects": [],
        "markers": [],
        "enabled": true,
        "metadata": {},
    })
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

/// Where a written timeline says its media are.
#[derive(Debug, Clone, Copy)]
pub enum Locations<'a> {
    /// At their absolute paths.
    Absolute,
    /// In a bundle: each media's path relative to the bundle, by media id
    /// (`media/A001.mov`; an image sequence's is the folder of its pictures,
    /// `media/renders/`).
    Bundle(&'a HashMap<String, String>),
}

/// The media's start timecode in whole frames at `fps` (0 without one; a
/// sound file's time of day to the nearest frame).
fn start_tc(m: &Media, fps: Rational) -> f64 {
    m.info.timecode.map_or(0.0, |tc| tc.frames_at(fps).round())
}

/// The OTIO media reference for `m`.
fn media_reference(p: &Project, m: &Media, locations: Locations, rate: f64) -> Value {
    let fps = p.settings.fps;
    let frames = p.media_frames(m) as f64;
    let bundled = match locations {
        Locations::Bundle(names) => names.get(&m.id).cloned(),
        Locations::Absolute => None,
    };
    if let Some(s) = &m.sequence {
        let base = bundled.unwrap_or_else(|| {
            let mut dir = m
                .path
                .parent()
                .unwrap_or(Path::new("/"))
                .to_string_lossy()
                .into_owned();
            if !dir.ends_with('/') {
                dir.push('/');
            }
            dir
        });
        let (prefix, padding, suffix) = s.parts();
        // OTIO counts a sequence's pictures as `available_range.duration
        // .to_frames(rate)`, truncated, at the reference's own rate: the
        // range is the picture count at the sequence rate, not the
        // timeline length at the project rate (the two differ whenever the
        // rates do).
        let pictures = m.info.video.as_ref().map_or(fps, |v| v.fps).as_f64();
        return json!({
            "OTIO_SCHEMA": "ImageSequenceReference.1",
            "name": m.name,
            "available_range": range(0.0, s.count as f64, pictures),
            "available_image_bounds": null,
            "target_url_base": base,
            "name_prefix": prefix,
            "name_suffix": suffix,
            "start_frame": s.first,
            "frame_step": 1,
            "rate": pictures,
            "frame_zero_padding": padding,
            "missing_frame_policy": "error",
            "metadata": {},
        });
    }
    let available = if m.is_still() {
        range(0.0, 1.0, rate)
    } else {
        range(start_tc(m, fps), frames, rate)
    };
    json!({
        "OTIO_SCHEMA": "ExternalReference.1",
        "name": m.name,
        "target_url": bundled.unwrap_or_else(|| m.path.to_string_lossy().into_owned()),
        "available_range": available,
        "available_image_bounds": null,
        "metadata": {},
    })
}

/// A clip volume effect at `db`, in the editor metadata.
fn editor_volume(db: f64) -> Value {
    json!({
        "OTIO_SCHEMA": "Effect.1",
        "name": "",
        "effect_name": "Resolve Effect",
        "metadata": { EDITOR: {
            "Effect Name": "Fairlight Clip Volume and Fades",
            "Enabled": true,
            "Name": "Volume",
            "Type": 62,
            "Parameters": [
                { "Parameter ID": "volume", "Parameter Value": db, "Variant Type": "Double" }
            ],
        } },
    })
}

/// The timeline range `[a, b)` of the clip's audio an export of `[a, b)`
/// writes, if any.
fn audio_part(c: &crate::model::Clip, a: i64, b: i64) -> Option<(i64, i64)> {
    let (ra, rb) = c.audio_range()?;
    let (s, e) = (ra.max(a), rb.min(b));
    (e > s).then_some((s, e))
}

/// The media an export of `[a, b)` (default: everything) names, in the
/// order clips first use them.
pub fn used_media(p: &Project, span: Option<(i64, i64)>) -> Vec<String> {
    let (a, b) = span.unwrap_or((0, p.end()));
    let mut used: Vec<String> = Vec::new();
    for t in &p.tracks {
        for c in &t.clips {
            let shows = c.video && c.start.max(a) < c.end().min(b);
            if (shows || audio_part(c, a, b).is_some()) && !used.contains(&c.media) {
                used.push(c.media.clone());
            }
        }
    }
    used
}

/// The timeline range `[a, b)` of `p` (default: all of it) as an OTIO
/// timeline named `name`, its media where `locations` says.
pub fn export(
    p: &Project,
    name: &str,
    span: Option<(i64, i64)>,
    locations: Locations,
) -> Result<Value> {
    let fps = p.settings.fps;
    let rate = fps.as_f64();
    let (a, b) = span.unwrap_or((0, p.end()));
    if b <= a {
        return Err(Error::invalid("nothing to export: the range is empty"));
    }
    let item = |label: &str,
                m: &Media,
                source: i64,
                frames: i64,
                enabled: bool,
                meta: Value,
                effects: Vec<Value>| {
        // Source positions count from the media's start timecode; stills
        // have one picture.
        let start = if m.is_still() {
            0.0
        } else if m.sequence.is_some() {
            source as f64
        } else {
            start_tc(m, fps) + source as f64
        };
        json!({
            "OTIO_SCHEMA": "Clip.2",
            "name": label,
            "source_range": range(start, frames as f64, rate),
            "media_references": {
                "DEFAULT_MEDIA": media_reference(p, m, locations, rate),
            },
            "active_media_reference_key": "DEFAULT_MEDIA",
            "effects": effects,
            "markers": [],
            "enabled": enabled,
            "metadata": meta,
        })
    };
    let track = |name: &str, kind: &str, children: Vec<Value>, enabled: bool, meta: Value| {
        json!({
            "OTIO_SCHEMA": "Track.1",
            "name": name,
            "kind": kind,
            "children": children,
            "source_range": null,
            "effects": [],
            "markers": [],
            "enabled": enabled,
            "metadata": meta,
        })
    };
    // A video track for each video track, and an audio track for each track
    // that carries audio; vidcut's track settings go in their metadata.
    let (mut video_tracks, mut audio_tracks) = (Vec::new(), Vec::new());
    let mut groups = 0;
    for t in &p.tracks {
        let (mut video, mut audio) = (Vec::new(), Vec::new());
        let (mut tv, mut ta) = (a, a);
        // The channel counts of the audio on this track.
        let mut channels: Vec<u16> = Vec::new();
        for c in &t.clips {
            let m = p
                .media(&c.media)
                .ok_or_else(|| Error::internal(format!("no media {}", c.media)))?;
            let label = c.name.clone().unwrap_or_else(|| m.name.clone());
            let (s, e) = (c.start.max(a), c.end().min(b));
            let shows = c.video && e > s;
            let sounds = audio_part(c, a, b).map(|(s, e)| (s.max(ta), e));
            let sounds = sounds.filter(|(s, e)| e > s);
            // A link group number joins a clip's video and audio.
            let group = (shows && sounds.is_some()).then(|| {
                groups += 1;
                groups
            });
            if shows {
                if s > tv {
                    video.push(gap(s - tv, rate));
                }
                let mut meta = json!({ "clip": c.id, "media": c.media });
                if let Some(au) = &c.audio {
                    meta["stream"] = json!(au.stream);
                    meta["gain_db"] = json!(au.gain_db);
                    meta["muted"] = json!(au.muted);
                }
                if m.is_still() {
                    meta["source_in"] = json!(c.source_at(s));
                }
                let mut metadata = json!({ VIDCUT: meta });
                if let Some(g) = group {
                    metadata[EDITOR] = json!({ "Link Group ID": g });
                }
                video.push(item(
                    &label,
                    m,
                    c.source_at(s),
                    e - s,
                    c.enabled,
                    metadata,
                    vec![],
                ));
                tv = e;
            }
            if let (Some(au), Some((s, e))) = (&c.audio, sounds) {
                if s > ta {
                    audio.push(gap(s - ta, rate));
                }
                let meta = json!({ "clip": c.id, "stream": au.stream, "gain_db": au.gain_db });
                let n = m
                    .info
                    .audio
                    .get(au.stream as usize)
                    .map_or(2, |x| x.channels);
                channels.push(n);
                let mut editor = json!({
                    "Channels": (0..n)
                        .map(|ch| json!({ "Source Channel ID": ch, "Source Track ID": au.stream }))
                        .collect::<Vec<_>>(),
                });
                if let Some(g) = group {
                    editor["Link Group ID"] = json!(g);
                }
                let effects = if au.gain_db == 0.0 {
                    vec![]
                } else {
                    vec![editor_volume(au.gain_db)]
                };
                audio.push(item(
                    &label,
                    m,
                    c.source_at(s),
                    e - s,
                    !au.muted,
                    json!({ VIDCUT: meta, EDITOR: editor }),
                    effects,
                ));
                ta = e;
            }
        }
        let meta = json!({
            "track": t.id, "locked": t.locked, "muted": t.muted, "solo": t.solo,
            "volume_db": t.volume_db, "video_enabled": t.video_enabled, "name": t.name,
        });
        let mut sound = json!({ "Locked": t.locked, "SoloOn": t.solo });
        if let Some(&n) = channels.first()
            && channels.iter().all(|&x| x == n)
            && let Some(kind) = match n {
                1 => Some("Mono"),
                2 => Some("Stereo"),
                _ => None,
            }
        {
            sound["Audio Type"] = json!(kind);
        }
        let sound = json!({ VIDCUT: meta, EDITOR: sound });
        if t.kind == TrackKind::Video {
            let picture = json!({ VIDCUT: meta, EDITOR: { "Locked": t.locked } });
            video_tracks.push(track(&t.id, "Video", video, t.video_enabled, picture));
            if !audio.is_empty() {
                let name = format!("{} audio", t.id);
                audio_tracks.push(track(&name, "Audio", audio, !t.muted, sound));
            }
        } else {
            audio_tracks.push(track(&t.id, "Audio", audio, !t.muted, sound));
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
                // Markers are one frame long.
                "marked_range": range((k.at - a) as f64, 1.0, rate),
                "metadata": { VIDCUT: { "color": k.color }, EDITOR: { "Note": k.note } },
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
        "global_start_time": rt((p.settings.start_tc + a) as f64, rate),
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
        "metadata": {
            VIDCUT: { "fps": p.settings.fps, "marks": marks },
            EDITOR: { "Resolve OTIO Meta Version": "1.0" },
        },
    }))
}

/// A clip's media file as an OTIO file names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MediaRef {
    /// As written: a URL or a path (for an image sequence, its first
    /// picture's).
    pub url: String,
    /// The path it names, to show.
    pub shown: String,
    /// Where to look for it, in order: the path it names (decoded, then as
    /// written), relative to the timeline file's folder if relative, then
    /// that folder joined with the path's last parts (a project folder that
    /// moved with the timeline file).
    pub paths: Vec<PathBuf>,
    /// Its file name, to look for in other folders.
    pub file_name: String,
    /// For an image sequence: the rate of its pictures.
    pub sequence: Option<Rational>,
}

/// A clip read from an OTIO file. Frames are at [`Imported::fps`].
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedClip {
    /// The track it goes on: `V<n>` or `A<n>`.
    pub track: String,
    pub media: MediaRef,
    pub name: String,
    pub start: i64,
    pub duration: i64,
    /// Frames from the media's first frame; with `timecoded`, from
    /// midnight of its timecode.
    pub source_in: i64,
    /// No available range says where the media start: the source range
    /// counts in the media's own timecode.
    pub timecoded: bool,
    /// False for audio-only clips.
    pub video: bool,
    pub enabled: bool,
    /// Audio offsets `(head, tail)` as in `ClipAudio`, if the clip has audio.
    pub audio: Option<(i64, i64)>,
    pub stream: u32,
    pub gain_db: f64,
    pub muted: bool,
    /// The link group joining a clip's video and audio.
    pub link: Option<i64>,
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
    /// From vidcut's metadata, else the editor metadata, else the OTIO track's
    /// `enabled`.
    fn of(otio: &Value, id: &str, audio: bool) -> Self {
        let meta = &otio["metadata"][VIDCUT];
        let editor = &otio["metadata"][EDITOR];
        let enabled = otio["enabled"].as_bool().unwrap_or(true);
        Self {
            id: id.to_string(),
            locked: meta["locked"]
                .as_bool()
                .or(editor["Locked"].as_bool())
                .unwrap_or(false),
            muted: meta["muted"].as_bool().unwrap_or(audio && !enabled),
            solo: meta["solo"]
                .as_bool()
                .or(editor["SoloOn"].as_bool().filter(|_| audio))
                .unwrap_or(false),
            volume_db: meta["volume_db"].as_f64().unwrap_or(0.0),
            video_enabled: meta["video_enabled"].as_bool().unwrap_or(audio || enabled),
            name: meta["name"].as_str().map(String::from),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
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

/// Frames at `fps` from OTIO time `from` to `to`, rounded once.
fn between(from: (f64, f64), to: (f64, f64), fps: Rational) -> i64 {
    if from.1 == to.1 {
        frames(to.0 - from.0, to.1, fps)
    } else {
        ((to.0 / to.1 - from.0 / from.1) * fps.as_f64()).round() as i64
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
fn media_reference_of(item: &Value) -> &Value {
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
        return time(&media_reference_of(item)["available_range"]["duration"])
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

/// `s` with `%XX` escapes decoded; `None` if that isn't UTF-8.
fn percent_decoded(s: &str) -> Option<String> {
    let hex = |b: u8| (b as char).to_digit(16);
    let bytes = s.as_bytes();
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
    String::from_utf8(out).ok()
}

/// A drive letter and colon (`C:`).
fn is_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// A Windows absolute path: `C:\…`, `C:/…` or `\\server\share\…`.
fn is_windows_absolute(s: &str) -> bool {
    s.starts_with("\\\\")
        || (s.len() > 2 && is_drive(&s[..2]) && matches!(s.as_bytes()[2], b'\\' | b'/'))
}

/// The first picture's name for an image sequence named by its range
/// (`frame_[0001-0120].png` → `frame_0001.png`).
fn bracketed(name: &str) -> Option<String> {
    let open = name.rfind('[')?;
    let close = open + name[open..].find(']')?;
    let (first, last) = name[open + 1..close].split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(first) && digits(last))
        .then(|| format!("{}{first}{}", &name[..open], &name[close + 1..]))
}

/// Where the media a URL or path names may be (see [`MediaRef`]); `base`
/// is the timeline file's folder. `None` for media on another computer.
fn locate(url: &str, base: Option<&Path>, fps: Rational) -> Option<MediaRef> {
    let written = url.trim();
    if written.is_empty() {
        return None;
    }
    // file:///p, file://localhost/p, file:/p and file:///C:/p are local;
    // OTIO's Python library leaves them unescaped. Plain paths are usually
    // not escaped either, but some tools do.
    let (first, second) = match written.strip_prefix("file:") {
        Some(rest) => {
            let path = match rest.strip_prefix("//") {
                None => rest,
                Some(r) if r.starts_with('/') => r,
                Some(r) => match r.strip_prefix("localhost") {
                    Some(r) if r.starts_with('/') => r,
                    _ if is_windows_absolute(r) => r,
                    _ => return None,
                },
            };
            let path = path
                .strip_prefix('/')
                .filter(|p| is_windows_absolute(p))
                .unwrap_or(path);
            (percent_decoded(path), Some(path.to_string()))
        }
        None if written.find("://").is_some_and(|i| i > 1) => return None,
        None => (Some(written.to_string()), percent_decoded(written)),
    };
    let mut forms: Vec<String> = Vec::new();
    for f in [first, second].into_iter().flatten() {
        if !forms.contains(&f) {
            forms.push(f);
        }
    }
    let mut shown = forms.first()?.clone();
    let mut file_name = shown.rsplit(['/', '\\']).next().unwrap_or("").to_string();
    let mut sequence = None;
    // An image sequence named by its range: its first picture.
    if let Some(first) = bracketed(&file_name) {
        for f in &mut forms {
            let dir = f.len() - f.rsplit(['/', '\\']).next().unwrap_or("").len();
            if let Some(name) = bracketed(&f[dir..]) {
                f.replace_range(dir.., &name);
            }
        }
        file_name = first;
        shown = forms[0].clone();
        sequence = Some(fps);
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    for f in &forms {
        let foreign = is_windows_absolute(f);
        if !foreign && !f.starts_with('/') {
            // Relative to the timeline file.
            if let Some(b) = base {
                paths.push(b.join(f));
                if f.contains('\\') {
                    paths.push(b.join(f.replace('\\', "/")));
                }
            }
            continue;
        }
        if !foreign {
            paths.push(PathBuf::from(f));
        }
        // Moved with the timeline file: its last parts next to the file.
        if let Some(b) = base {
            let parts: Vec<&str> = f
                .split(['/', '\\'])
                .filter(|s| !s.is_empty() && !is_drive(s))
                .collect();
            for k in (1..=parts.len().min(3)).rev() {
                paths.push(b.join(parts[parts.len() - k..].join("/")));
            }
        }
    }
    let mut unique: Vec<PathBuf> = Vec::new();
    for p in paths {
        if !unique.contains(&p) {
            unique.push(p);
        }
    }
    Some(MediaRef {
        url: written.to_string(),
        shown,
        paths: unique,
        file_name,
        sequence,
    })
}

/// The media of an `ImageSequenceReference`: its first picture.
fn sequence_ref(r: &Value, base: Option<&Path>, fps: Rational) -> Result<MediaRef> {
    let bad = |why: &str| Error::media(format!("an image sequence {why}"));
    let url_base = r["target_url_base"]
        .as_str()
        .ok_or_else(|| bad("has no folder"))?;
    if r["frame_step"].as_i64().unwrap_or(1) != 1 {
        return Err(bad("skips numbers (frame_step)"));
    }
    let start = r["start_frame"].as_i64().unwrap_or(1);
    let padding = r["frame_zero_padding"].as_u64().unwrap_or(0) as usize;
    let first = format!(
        "{}{start:0padding$}{}",
        r["name_prefix"].as_str().unwrap_or(""),
        r["name_suffix"].as_str().unwrap_or("")
    );
    let mut url = url_base.to_string();
    if !url.is_empty() && !url.ends_with(['/', '\\']) {
        url.push('/');
    }
    url.push_str(&first);
    let rate = r["rate"].as_f64().and_then(|x| rational(x).ok());
    let mut m = locate(&url, base, fps).ok_or_else(|| bad(&format!("at {url_base:?}")))?;
    m.sequence = Some(rate.unwrap_or(fps));
    Ok(m)
}

/// A clip's media and source start, in frames from the media's first
/// frame. Source ranges count from the available range's start, which
/// editors set to the media's timecode; without one, from the timecode's
/// midnight (the flag: the importer subtracts the media's timecode).
fn clip_source(item: &Value, fps: Rational, base: Option<&Path>) -> Result<(MediaRef, i64, bool)> {
    let reference = media_reference_of(item);
    let name = item["name"].as_str().unwrap_or("");
    let s = schema(reference);
    let unsupported =
        |url: &str| Error::media(format!("clip {name:?}: unsupported media URL {url:?}"));
    let media = if s.starts_with("ExternalReference") {
        let url = reference["target_url"].as_str().unwrap_or("");
        locate(url, base, fps).ok_or_else(|| unsupported(url))?
    } else if s.starts_with("ImageSequenceReference") {
        sequence_ref(reference, base, fps)
            .map_err(|e| Error::media(format!("clip {name:?}: {}", e.message)))?
    } else if let Some(url) = reference["metadata"][EDITOR]["Media Url"]
        .as_str()
        .filter(|_| s.starts_with("MissingReference"))
    {
        // Media the editor had offline: they may be here.
        locate(url, base, fps).ok_or_else(|| unsupported(url))?
    } else {
        return Err(Error::media(format!(
            "clip {name:?} has no media file ({})",
            match s {
                "" => "no reference",
                s => s,
            }
        )));
    };
    let available = time(&reference["available_range"]["start_time"]);
    Ok(
        match (time(&item["source_range"]["start_time"]), available) {
            (Some(start), Some(base)) => (media, between(base, start, fps), false),
            (Some((v, r)), None) => (media, frames(v, r, fps), true),
            (None, _) => (media, 0, false),
        },
    )
}

/// The link group of a clip.
fn link_group(item: &Value) -> Option<i64> {
    item["metadata"][EDITOR]["Link Group ID"].as_i64()
}

/// The audio stream the channel map plays.
fn editor_stream(item: &Value) -> Option<u32> {
    let channels = item["metadata"][EDITOR]["Channels"].as_array()?;
    let track = channels.first()?["Source Track ID"].as_u64()?;
    u32::try_from(track).ok()
}

/// The clip volume in dB, noting what of it is left out.
fn editor_gain(item: &Value, warnings: &mut Vec<String>) -> Option<f64> {
    for e in item["effects"].as_array().into_iter().flatten() {
        let r = &e["metadata"][EDITOR];
        if r["Effect Name"] != "Fairlight Clip Volume and Fades" || r["Enabled"] == false {
            continue;
        }
        let mut gain = None;
        for p in r["Parameters"].as_array().into_iter().flatten() {
            let value = p["Parameter Value"].as_f64();
            match p["Parameter ID"].as_str() {
                Some("volume") => {
                    gain = value;
                    if p["Key Frames"].as_object().is_some_and(|k| !k.is_empty()) {
                        warn(
                            warnings,
                            "clip volume keyframes were left out (the clips keep their base level)",
                        );
                    }
                }
                Some("faderIn" | "faderOut") if value.is_some_and(|v| v > 0.0) => {
                    warn(warnings, "audio fades were left out")
                }
                _ => {}
            }
        }
        // Editors write -16.999998 for -17.
        return gain.map(|g| (g * 100.0).round() / 100.0);
    }
    None
}

/// Notes the effects of `item` that are left out.
fn note_effects(item: &Value, warnings: &mut Vec<String>) {
    for e in item["effects"].as_array().into_iter().flatten() {
        let s = schema(e);
        if s.starts_with("LinearTimeWarp")
            || s.starts_with("FreezeFrame")
            || s.starts_with("TimeEffect")
        {
            warn(
                warnings,
                "speed changes were left out (those clips play at normal speed)",
            );
        }
    }
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
                "nested stacks and tracks (compound clips) were left out (gaps instead)",
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

/// The timeline's rate: the one most of its items are measured in, else its
/// start time's.
fn timeline_rate(tl: &Value, tracks: &[&Value]) -> Option<f64> {
    let mut counts: Vec<(f64, usize)> = Vec::new();
    for item in tracks
        .iter()
        .flat_map(|t| t["children"].as_array().into_iter().flatten())
    {
        if let Some((_, r)) = time(&item["source_range"]["duration"]) {
            match counts.iter_mut().find(|(x, _)| (x - r).abs() < 1e-6) {
                Some((_, n)) => *n += 1,
                None => counts.push((r, 1)),
            }
        }
    }
    let most = counts.iter().max_by_key(|(_, n)| *n).map(|(r, _)| *r);
    most.or_else(|| time(&tl["global_start_time"]).map(|(_, r)| r))
}

/// A marker at timeline frame `at`.
fn marker(k: &Value, at: i64) -> ImportedMarker {
    let color = match k["metadata"][VIDCUT]["color"].as_str() {
        Some(c) if crate::ops::MARKER_COLORS.contains(&c) => c.to_string(),
        _ => our_color(k["color"].as_str().unwrap_or("")).to_string(),
    };
    // Marker notes may be in the editor metadata.
    let note = k["comment"]
        .as_str()
        .filter(|c| !c.is_empty())
        .or(k["metadata"][EDITOR]["Note"].as_str())
        .unwrap_or("");
    ImportedMarker {
        at,
        name: k["name"].as_str().unwrap_or("").to_string(),
        note: note.to_string(),
        color,
    }
}

/// The markers on a clip that starts at timeline frame `start` and lasts
/// `len`: their times count like its source range.
fn clip_markers(item: &Value, start: i64, len: i64, fps: Rational, out: &mut Vec<ImportedMarker>) {
    let source = time(&item["source_range"]["start_time"]);
    for k in item["markers"].as_array().into_iter().flatten() {
        let Some(t) = time(&k["marked_range"]["start_time"]) else {
            continue;
        };
        let offset = match source {
            Some(s) => between(s, t, fps),
            None => frames(t.0, t.1, fps),
        };
        if (0..len).contains(&offset) {
            out.push(marker(k, start + offset));
        }
    }
}

/// Reads an OTIO document (the parsed JSON of an `.otio` file). Relative
/// media paths are relative to `base`, the file's folder.
pub fn import(doc: &Value, base: Option<&Path>) -> Result<Imported> {
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
    // The rate: ours if we wrote the file, else the one most items have.
    let ours = tl["metadata"][VIDCUT]["fps"]
        .as_str()
        .and_then(|s| s.parse::<Rational>().ok());
    let fps = match ours {
        Some(r) => r,
        None => rational(
            timeline_rate(tl, &tracks)
                .ok_or_else(|| Error::usage("this OTIO file has no frame rate"))?,
        )?,
    };
    let start_tc = time(&tl["global_start_time"]).map_or(0, |(v, r)| frames(v, r, fps));
    let mut markers = Vec::new();

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
            note_effects(item, &mut warnings);
            match clip_source(item, fps, base) {
                Ok((media, source_in, timecoded)) => {
                    let meta = &item["metadata"][VIDCUT];
                    clip_markers(item, start, len, fps, &mut markers);
                    clips.push(ImportedClip {
                        track: id.clone(),
                        media,
                        name: item["name"].as_str().unwrap_or("").to_string(),
                        start,
                        duration: len,
                        // A still's, which its source range doesn't keep.
                        source_in: meta["source_in"].as_i64().unwrap_or(source_in),
                        timecoded: timecoded && meta["source_in"].is_null(),
                        video: true,
                        enabled: item["enabled"].as_bool().unwrap_or(true),
                        audio: None,
                        stream: meta["stream"].as_u64().unwrap_or(0) as u32,
                        gain_db: meta["gain_db"].as_f64().unwrap_or(0.0),
                        muted: meta["muted"].as_bool().unwrap_or(false),
                        link: link_group(item),
                    });
                }
                Err(e) => warnings.push(format!("{}; left as a gap", e.message)),
            }
        }
    }
    // An audio clip lining up with a video clip of the same media (on the
    // track vidcut named, else on any; in its link group if both
    // have one) is that clip's audio. Others are audio-only clips: on the
    // track vidcut named, else on audio tracks A1, A2… in the file's order.
    let mut foreign_audio = 0;
    for a in &audio_tracks {
        let hint = a["metadata"][VIDCUT]["track"].as_str().map(String::from);
        let mut own: Option<String> = None;
        for (start, len, item) in walk(a, fps, &mut warnings) {
            if !is_clip(item) || len <= 0 {
                continue;
            }
            note_effects(item, &mut warnings);
            let (media, source_in, timecoded) = match clip_source(item, fps, base) {
                Ok(s) => s,
                Err(e) => {
                    warnings.push(format!("{}; left as a gap", e.message));
                    continue;
                }
            };
            clip_markers(item, start, len, fps, &mut markers);
            let span = (start, start + len);
            let meta = &item["metadata"][VIDCUT];
            let muted = item["enabled"].as_bool() == Some(false);
            let link = link_group(item);
            let gain = meta["gain_db"]
                .as_f64()
                .or_else(|| editor_gain(item, &mut warnings));
            let stream = meta["stream"]
                .as_u64()
                .map(|s| s as u32)
                .or_else(|| editor_stream(item));
            let pairs = |c: &ImportedClip| {
                c.video
                    && c.audio.is_none()
                    && c.media.url == media.url
                    && c.timecoded == timecoded
                    && c.source_in - c.start == source_in - start
                    && overlaps((c.start, c.end()), span)
                    && match (c.link, link) {
                        (Some(x), Some(y)) => x == y,
                        _ => true,
                    }
            };
            let owner = match &hint {
                Some(h) if h.starts_with('A') => None,
                Some(h) => clips.iter_mut().find(|c| &c.track == h && pairs(c)),
                None => clips.iter_mut().find(|c| pairs(c)),
            };
            if let Some(c) = owner {
                c.audio = Some((start - c.start, span.1 - c.end()));
                c.muted |= muted;
                if let Some(g) = gain {
                    c.gain_db = g;
                }
                if let Some(s) = stream {
                    c.stream = s;
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
                media,
                name: item["name"].as_str().unwrap_or("").to_string(),
                start,
                duration: len,
                source_in,
                timecoded,
                video: false,
                enabled: true,
                audio: Some((0, 0)),
                stream: stream.unwrap_or(0),
                gain_db: gain.unwrap_or(0.0),
                muted,
                link,
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

    // Markers of the timeline and of its tracks (tracks start with it).
    for list in
        std::iter::once(&tl["tracks"]["markers"]).chain(tracks.iter().map(|t| &t["markers"]))
    {
        for k in list.as_array().into_iter().flatten() {
            if let Some((v, r)) = time(&k["marked_range"]["start_time"]) {
                markers.push(marker(k, frames(v, r, fps)));
            }
        }
    }
    markers.retain(|k| k.at >= 0);
    // A clip's video and audio may carry the same markers.
    markers.sort();
    markers.dedup();
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
