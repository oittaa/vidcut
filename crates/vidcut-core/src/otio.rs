//! OpenTimelineIO interchange (deviations.md X9): writes the timeline as
//! OTIO and reads OTIO back, in the shapes other editors write and read.
//!
//! **Writing** ([`export`]):
//! - Each video track becomes an OTIO video track of clips and gaps.
//! - The audio of each track's clips becomes an audio track, and each audio
//!   track one more. Each audio clip covers its clip's audio range, so J/L
//!   cuts show up as audio edits offset from the video edits, which is what
//!   other editors expect.
//! - Tracks carry the names given to them (a track's audio its track's),
//!   else the ones editors give their own by place: `Video 1`, `Audio 2`.
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
//! - The clip effects a clip carries from another editor's file, as they
//!   came ([`effects`]), and the transitions on its edit points that still
//!   fit there ([`transitions`]). A generator clip's reference, as it came.
//! - Timeline markers go on the top-level stack, with their lengths.
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
//! - Track names other than editors' defaults (`Video 1`, `Audio 2`) name
//!   the tracks, unless vidcut's metadata says otherwise.
//! - Video tracks become V1, V2, … An audio clip that lines up with a video
//!   clip of the same media (on any video track, in the same link group if
//!   it has one) becomes that clip's audio, J/L offsets included.
//!   Other audio clips go on audio tracks A1, A2, … (files vidcut wrote say
//!   which track each audio track belongs to). The editor metadata's channel
//!   map picks the stream, its clip volume the gain.
//! - Markers on the timeline, on tracks and on clips become timeline
//!   markers, keeping their lengths.
//! - Clip effects in use stay on their clips' pictures and sounds
//!   ([`effects`]), not shown or played, and are written back. So do
//!   transitions, on their edit points ([`transitions`]): shown as cuts.
//! - Generators and titles (`GeneratorReference`s) become generator clips,
//!   which play as gaps, and nested `Stack`s (compound clips, nested
//!   timelines) compound clips, which play the clips they hold: their
//!   references and stacks kept.
//! - Speed changes (time effects) stay on their clips, which play and are
//!   edited at them (decision D48).
//! - Everything else is left out with a warning: nested tracks (gaps
//!   instead).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{Clip, Compound, Marks, Media, Project, Speed, TrackKind};
use crate::store::{absolute_elsewhere, is_drive, windows_absolute};
use crate::time::Rational;

pub mod effects;
#[cfg(test)]
mod tests;
pub mod transitions;

/// Where vidcut keeps its own details in OTIO metadata.
const VIDCUT: &str = "vidcut";
/// Where other editors keep their own details: the key and values are
/// theirs, so that they read what vidcut writes.
const EDITOR: &str = "Resolve_OTIO";
/// The import warning naming the compound clips it kept.
pub const COMPOUNDS_KEPT: &str =
    "compound clips and nested timelines kept as blocks, playing their clips";

/// `small`, an object, with `big` moved in: `json!` serializes what it is
/// given, so a `Value` put in it is copied whole.
fn with(mut small: Value, big: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    for (key, value) in big {
        small[key] = value;
    }
    small
}

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

/// Clip Color `name` as OTIO writes an item's color.
fn otio_clip_color(name: &str) -> Option<Value> {
    let (n, [r, g, b]) = crate::ops::CLIP_COLORS.iter().find(|(n, _)| *n == name)?;
    let mut title = n.to_string();
    title[..1].make_ascii_uppercase();
    let x = |v: &u8| f64::from(*v) / 255.0;
    Some(json!({
        "OTIO_SCHEMA": "Color.1",
        "name": title,
        "r": x(r),
        "g": x(g),
        "b": x(b),
        "a": 1.0,
    }))
}

/// The Clip Color of an OTIO item: its color's name if it is one of ours,
/// else ours nearest to it.
fn clip_color(item: &Value) -> Option<String> {
    let c = &item["color"];
    let name = c["name"].as_str().unwrap_or("").to_ascii_lowercase();
    let colors = crate::ops::CLIP_COLORS;
    if let Some((n, _)) = colors.iter().find(|(n, _)| *n == name) {
        return Some(n.to_string());
    }
    let rgb = [&c["r"], &c["g"], &c["b"]].map(|v| v.as_f64().map(|v| v * 255.0));
    let [Some(r), Some(g), Some(b)] = rgb else {
        return None;
    };
    let off = |[x, y, z]: &[u8; 3]| {
        (f64::from(*x) - r).powi(2) + (f64::from(*y) - g).powi(2) + (f64::from(*z) - b).powi(2)
    };
    colors
        .iter()
        .min_by(|a, b| off(&a.1).total_cmp(&off(&b.1)))
        .map(|(n, _)| n.to_string())
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
            if !dir.ends_with(['/', std::path::MAIN_SEPARATOR]) {
                dir.push(std::path::MAIN_SEPARATOR);
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

/// Where the file of `m` is, as `locations` names it: in a bundle, its
/// place there.
fn written_path(m: &Media, locations: Locations) -> PathBuf {
    match locations {
        Locations::Bundle(names) => match (names.get(&m.id), &m.sequence) {
            (Some(dir), Some(_)) => PathBuf::from(dir).join(m.path.file_name().unwrap_or_default()),
            (Some(path), None) => PathBuf::from(path),
            (None, _) => m.path.clone(),
        },
        Locations::Absolute => m.path.clone(),
    }
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
    let mut seen: HashSet<&str> = HashSet::new();
    for t in &p.tracks {
        for c in &t.clips {
            let shows = c.video && c.start.max(a) < c.end().min(b);
            if (shows || audio_part(c, a, b).is_some()) && !c.is_block() && seen.insert(&c.media) {
                used.push(c.media.clone());
            }
        }
    }
    used
}

/// A speed change: its time `effect`.
fn speed_of(effect: Option<Value>) -> Option<Speed> {
    effect.map(|effect| Speed { effect, skip: 0 })
}

/// How fast a speed change goes (1 without one).
fn scalar(speed: Option<&Speed>) -> f64 {
    speed.and_then(Speed::scalar).unwrap_or(1.0)
}

/// The first source frame shown by an item whose source range starts at
/// `start`, `len` frames at `speed`, and how far into its pace it starts:
/// backward, editors write the bottom of the range it walks down, and its
/// first frame shows the bottom of its stretch at the top (decision D48).
fn first_shown(start: i64, len: i64, speed: f64) -> (i64, i64) {
    if speed >= 0.0 {
        return (start, 0);
    }
    let bottom = start as f64 + (len - 1) as f64 * -speed;
    let first = (bottom + 1e-9).floor() as i64;
    let phase = bottom - first as f64;
    let skip = (0..1000)
        .find(|&k| (crate::render::Pace::of(speed, k).phase() - phase).abs() < 1e-6)
        .unwrap_or(0);
    (first, skip)
}

/// The source range start written for clip `c`'s part from timeline frame
/// `s`, `len` frames long: [`first_shown`]'s inverse.
fn written_start(c: &Clip, s: i64, len: i64) -> i64 {
    let first = c.source_at(s);
    match c.speed() {
        x if x < 0.0 => {
            let top = first as f64 + c.pace().after(s - c.start).sound(0.0);
            (top + len as f64 * x).round() as i64
        }
        _ => first,
    }
}

/// The editor's `Media In` for clip `c`'s part from timeline frame `s`
/// walking backward: the room above its top, in seconds of timeline, which
/// the editor places it by.
fn media_in(p: &Project, c: &Clip, m: Option<&Media>, s: i64) -> Option<f64> {
    let x = c.speed();
    if x >= 0.0 {
        return None;
    }
    let top = c.source_at(s) as f64 + c.pace().after(s - c.start).sound(0.0);
    let frames = p.media_limit(m?) as f64;
    Some((frames - top) / -x / p.settings.fps.as_f64())
}

/// vidcut's own record of where clip `c`'s part from timeline frame `s`
/// starts in its source and its pace, at another speed: exact where the
/// source range rounds.
fn own_pace(meta: &mut Value, c: &Clip, s: i64) {
    if let Some(speed) = &c.speed {
        meta["source_in"] = json!(c.source_at(s));
        meta["skip"] = json!(speed.skip + s - c.start);
    }
}

/// `effects`, after clip `c`'s speed change if it carries one: editors
/// write the time effect first.
fn with_speed(c: &Clip, mut effects: Vec<Value>) -> Vec<Value> {
    if let Some(speed) = &c.speed {
        effects.insert(0, speed.effect.clone());
    }
    effects
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
                c: &Clip,
                m: Option<&Media>,
                source: i64,
                frames: i64,
                enabled: bool,
                meta: Value,
                effects: Vec<Value>| {
        // Source positions count from the media's start timecode; stills
        // have one picture. A generator's reference is written as it came.
        let (start, reference) = match m {
            Some(m) if m.is_still() => (0.0, media_reference(p, m, locations, rate)),
            Some(m) if m.sequence.is_some() => {
                (source as f64, media_reference(p, m, locations, rate))
            }
            Some(m) => (
                start_tc(m, fps) + source as f64,
                media_reference(p, m, locations, rate),
            ),
            None => (
                source as f64,
                c.generator
                    .as_ref()
                    .map_or(Value::Null, |g| g.reference.clone()),
            ),
        };
        with(
            json!({
                "OTIO_SCHEMA": "Clip.2",
                "name": label,
                "active_media_reference_key": "DEFAULT_MEDIA",
                "markers": [],
                "enabled": enabled,
            }),
            [
                ("source_range", range(start, frames as f64, rate)),
                (
                    "media_references",
                    with(json!({}), [("DEFAULT_MEDIA", reference)]),
                ),
                ("effects", Value::Array(effects)),
                ("metadata", meta),
            ],
        )
    };
    let track = |name: &str, kind: &str, children: Vec<Value>, enabled: bool, meta: Value| {
        with(
            json!({
                "OTIO_SCHEMA": "Track.1",
                "name": name,
                "kind": kind,
                "source_range": null,
                "effects": [],
                "markers": [],
                "enabled": enabled,
            }),
            [("children", Value::Array(children)), ("metadata", meta)],
        )
    };
    // Each clip's media, found once.
    let media: HashMap<&str, &Media> = p.media.iter().map(|m| (m.id.as_str(), m)).collect();
    // The Clip Color clip `c` shows: its own, else its media's.
    let shown_color = |c: &Clip| {
        c.color
            .as_deref()
            .or_else(|| media.get(c.media.as_str()).and_then(|m| m.color.as_deref()))
            .and_then(otio_clip_color)
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
        let edits = transitions::Edits::of(t);
        for c in &t.clips {
            let m = media.get(c.media.as_str()).copied();
            if m.is_none() && !c.is_block() {
                return Err(Error::internal(format!("no media {}", c.media)));
            }
            let label = c
                .name
                .clone()
                .or_else(|| m.map(|m| m.name.clone()))
                .unwrap_or_default();
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
                if let Some(x) = transitions::head(p, &edits, c, false, s, &video, rate) {
                    video.push(x);
                }
                let mut meta = json!({ "clip": c.id, "media": c.media, "color": c.color });
                if let Some(au) = &c.audio {
                    meta["stream"] = json!(au.stream);
                    meta["gain_db"] = json!(au.gain_db);
                    meta["muted"] = json!(au.muted);
                }
                if m.is_some_and(Media::is_still) {
                    meta["source_in"] = json!(c.source_at(s));
                }
                own_pace(&mut meta, c, s);
                let mut metadata = with(json!({}), [(VIDCUT, meta)]);
                if let Some(g) = group {
                    metadata[EDITOR] = json!({ "Link Group ID": g });
                }
                if let Some(x) = media_in(p, c, m, s) {
                    metadata[EDITOR]["Media In"] = json!(x);
                }
                // Carried effects, keyframes moved to where the item
                // starts, fades cut where the export does.
                let effects = with_speed(
                    c,
                    c.effects.as_ref().map_or_else(Vec::new, |x| {
                        x.written(c.source_at(s), s > c.start, e < c.end())
                    }),
                );
                let mut it = match &c.compound {
                    Some(k) => compound_item(c, k, &label, c.source_at(s), e - s, effects, rate),
                    None => item(
                        &label,
                        c,
                        m,
                        written_start(c, s, e - s),
                        e - s,
                        c.enabled,
                        metadata,
                        effects,
                    ),
                };
                if let Some(x) = shown_color(c) {
                    it["color"] = x;
                }
                video.push(it);
                if let Some(x) = transitions::tail(p, &edits, c, false, e, rate) {
                    video.push(x);
                }
                tv = e;
            }
            if let (Some(au), Some((s, e))) = (&c.audio, sounds) {
                if s > ta {
                    audio.push(gap(s - ta, rate));
                }
                if let Some(x) = transitions::head(p, &edits, c, true, s, &audio, rate) {
                    audio.push(x);
                }
                let mut meta = json!({
                    "clip": c.id, "media": c.media, "color": c.color, "stream": au.stream,
                    "gain_db": au.gain_db,
                });
                own_pace(&mut meta, c, s);
                let n = m
                    .and_then(|m| m.info.audio.get(au.stream as usize))
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
                if let Some(x) = media_in(p, c, m, s) {
                    editor["Media In"] = json!(x);
                }
                let whole = c.audio_range().expect("has audio");
                let effects = with_speed(
                    c,
                    effects::sound(au, c.source_at(s), s > whole.0, e < whole.1),
                );
                let mut it = item(
                    &label,
                    c,
                    m,
                    written_start(c, s, e - s),
                    e - s,
                    !au.muted,
                    with(json!({}), [(VIDCUT, meta), (EDITOR, editor)]),
                    effects,
                );
                if let Some(x) = shown_color(c) {
                    it["color"] = x;
                }
                audio.push(it);
                if let Some(x) = transitions::tail(p, &edits, c, true, e, rate) {
                    audio.push(x);
                }
                ta = e;
            }
            // A compound clip's sound, on its audio track.
            if let Some(k) = c.compound.as_ref().filter(|_| !c.video) {
                let (s, e) = (c.start.max(a).max(ta), c.end().min(b));
                if e > s {
                    if s > ta {
                        audio.push(gap(s - ta, rate));
                    }
                    let effects = with_speed(
                        c,
                        c.effects.as_ref().map_or_else(Vec::new, |x| {
                            x.written(c.source_at(s), s > c.start, e < c.end())
                        }),
                    );
                    audio.push(compound_item(
                        c,
                        k,
                        &label,
                        c.source_at(s),
                        e - s,
                        effects,
                        rate,
                    ));
                    ta = e;
                }
            }
        }
        transitions::close(&mut video, rate);
        transitions::close(&mut audio, rate);
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
        // The track's name, else the one editors give their own tracks by
        // place: `Video 1`, `Audio 2`. A track's audio takes its name too.
        let named = |kind: &str, n: usize| t.name.clone().unwrap_or(format!("{kind} {n}"));
        if t.kind == TrackKind::Video {
            let picture = json!({ VIDCUT: meta, EDITOR: { "Locked": t.locked } });
            let name = named("Video", video_tracks.len() + 1);
            video_tracks.push(track(&name, "Video", video, t.video_enabled, picture));
            if !audio.is_empty() {
                let name = named("Audio", audio_tracks.len() + 1);
                audio_tracks.push(track(&name, "Audio", audio, !t.muted, sound));
            }
        } else {
            let name = named("Audio", audio_tracks.len() + 1);
            audio_tracks.push(track(&name, "Audio", audio, !t.muted, sound));
        }
    }
    let markers: Vec<Value> = p
        .markers
        .iter()
        // A range takes the markers that start in it, cut at its end; the
        // whole timeline takes them all, each its length (spec D40).
        .filter(|k| span.is_none() || (a..b).contains(&k.at))
        .map(|k| {
            let length = match span {
                Some(_) => k.duration.min(b - k.at),
                None => k.duration,
            };
            json!({
                "OTIO_SCHEMA": "Marker.2",
                "name": k.name,
                "comment": k.note,
                "color": otio_color(&k.color),
                "marked_range": range((k.at - a) as f64, length as f64, rate),
                "metadata": {
                    VIDCUT: { "marker": k.id, "color": k.color },
                    EDITOR: { "Note": k.note },
                },
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
    let mut ours = json!({ "fps": p.settings.fps, "marks": marks });
    // The whole timeline is the project: the rest of it goes along.
    if span.is_none() {
        ours["project"] = json!(ProjectMeta {
            settings: p.settings.clone(),
            media: p
                .media
                .iter()
                .map(|m| Media {
                    path: written_path(m, locations),
                    ..m.clone()
                })
                .collect(),
            bins: p.bins.clone(),
            tape_marks: p.tape_marks.clone(),
            next_id: p.next_id.clone(),
        });
    }
    let tracks = with(
        json!({
            "OTIO_SCHEMA": "Stack.1",
            "name": "tracks",
            "source_range": null,
            "effects": [],
            "enabled": true,
            "metadata": {},
        }),
        [
            (
                "children",
                Value::Array(video_tracks.into_iter().chain(audio_tracks).collect()),
            ),
            ("markers", Value::Array(markers)),
        ],
    );
    Ok(with(
        json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": name,
            "global_start_time": rt((p.settings.start_tc + a) as f64, rate),
        }),
        [
            ("tracks", tracks),
            (
                "metadata",
                with(
                    json!({ EDITOR: { "Resolve OTIO Meta Version": "1.0" } }),
                    [(VIDCUT, ours)],
                ),
            ),
        ],
    ))
}

/// Nested `Stack` `item` as a compound clip: the stack without its source
/// range, clip effects and markers (the clip's own) and its link group
/// (whose number another editor gave); its length, the longest of its
/// tracks, at `fps`.
fn compound_of(item: &Value, fps: Rational) -> Compound {
    let frames = item["children"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            t["children"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|i| !schema(i).starts_with("Transition"))
                .map(|i| item_len(i, fps))
                .sum::<i64>()
        })
        .max()
        .unwrap_or(0);
    let mut stack = item.clone();
    if let Some(o) = stack.as_object_mut() {
        for key in ["source_range", "effects", "markers"] {
            o.remove(key);
        }
    }
    if let Some(m) = stack
        .pointer_mut(&format!("/metadata/{EDITOR}"))
        .and_then(Value::as_object_mut)
    {
        m.remove("Link Group ID");
    }
    Compound {
        stack,
        frames,
        tracks: Vec::new(),
    }
}

/// Compound clip `c`'s OTIO item: its stack, showing `frames` from
/// `source`.
fn compound_item(
    c: &Clip,
    k: &Compound,
    label: &str,
    source: i64,
    frames: i64,
    effects: Vec<Value>,
    rate: f64,
) -> Value {
    let mut v = k.stack.clone();
    v["name"] = json!(label);
    v["source_range"] = range(source as f64, frames as f64, rate);
    v["effects"] = Value::Array(effects);
    v["markers"] = json!([]);
    v["enabled"] = json!(c.enabled);
    v["metadata"][VIDCUT] = json!({ "clip": c.id, "color": c.color });
    v
}

/// What compound `k` is, as its stack says: `Compound Clip`, `Timeline
/// Clip`.
pub fn compound_kind(k: &Compound) -> String {
    k.stack["metadata"][EDITOR]["Sequence Type"]
        .as_str()
        .filter(|n| !n.is_empty())
        .unwrap_or("compound")
        .to_string()
}

/// What generator `g` makes, as its reference says: `Solid Color`, `Rich`.
pub fn generator_kind(g: &crate::model::Generator) -> String {
    let r = &g.reference;
    [&r["generator_kind"], &r["name"]]
        .into_iter()
        .filter_map(Value::as_str)
        .find(|n| !n.is_empty())
        .unwrap_or("generator")
        .to_string()
}

/// A clip's media file as an OTIO file names it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
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
    /// Which clip it is, for [`ImportedTransition`]s.
    pub key: usize,
    /// Its id and its media's in vidcut, when vidcut wrote the file.
    pub id: Option<String>,
    pub media_id: Option<String>,
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
    /// The clip effects in use on its picture and on its sound
    /// ([`effects`]), as the file has them.
    pub effects: Vec<Value>,
    pub audio_effects: Vec<Value>,
    /// A generator or title: its `GeneratorReference`, as the file has
    /// it. It has no media (`media` is empty).
    pub generator: Option<Value>,
    /// A compound clip or nested timeline: its `Stack`, frames at
    /// [`Imported::fps`]. It has no media; on an audio track, no picture.
    pub compound: Option<Compound>,
    /// Its speed change: a time effect, as the file has it.
    pub speed: Option<Speed>,
    /// The Clip Color it shows, and its own as vidcut wrote it
    /// ([`kept_color`]).
    pub color: Option<String>,
    pub own_color: Option<Option<String>>,
}

impl ImportedClip {
    pub fn end(&self) -> i64 {
        self.start + self.duration
    }

    /// Its content moves on `k` frames of its pace, as [`Clip::walk`].
    pub fn walk(&mut self, k: i64) {
        let skip = self.speed.as_ref().map_or(0, |s| s.skip);
        let speed = self.speed.as_ref().and_then(Speed::scalar).unwrap_or(1.0);
        self.source_in += crate::render::Pace::of(speed, skip).offset(k);
        if let Some(s) = self.speed.as_mut() {
            s.skip += k;
        }
    }

    /// Whether it is a generator or compound clip, without media.
    pub fn is_block(&self) -> bool {
        self.generator.is_some() || self.compound.is_some()
    }

    /// A block without media (`generator` or `compound` set by the
    /// caller) for OTIO `item` on `track`, at `start` for `len` frames.
    fn block(
        key: usize,
        track: &str,
        item: &Value,
        (start, len): (i64, i64),
        fps: Rational,
    ) -> Self {
        Self {
            key,
            id: our_str(item, "clip"),
            media_id: None,
            track: track.to_string(),
            media: MediaRef::default(),
            name: item["name"].as_str().unwrap_or("").to_string(),
            start,
            duration: len,
            source_in: time(&item["source_range"]["start_time"])
                .map_or(0, |(v, r)| frames(v, r, fps)),
            timecoded: false,
            video: true,
            enabled: item["enabled"].as_bool().unwrap_or(true),
            audio: None,
            stream: 0,
            gain_db: 0.0,
            muted: false,
            link: None,
            effects: Vec::new(),
            audio_effects: Vec::new(),
            generator: None,
            compound: None,
            speed: None,
            color: clip_color(item),
            own_color: kept_color(item),
        }
    }

    fn audio_range(&self) -> Option<(i64, i64)> {
        self.audio.map(|(h, t)| (self.start + h, self.end() + t))
    }
}

/// A transition read from an OTIO file, between the clips with these
/// [`ImportedClip::key`]s (none: a gap). Frames are at [`Imported::fps`].
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedTransition {
    /// Between sounds, not pictures.
    pub sound: bool,
    pub from: Option<usize>,
    pub to: Option<usize>,
    pub before: i64,
    pub after: i64,
    /// As the file has it.
    pub otio: Value,
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
    /// `enabled` and `name` (unless an editor's default name).
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
            name: match meta.get("name") {
                Some(name) => name.as_str().map(String::from),
                None => otio["name"]
                    .as_str()
                    .filter(|n| !default_track_name(n))
                    .map(String::from),
            },
        }
    }
}

/// A name editors give a track by place, not one somebody gave it: `Video 1`,
/// `Audio 2`, `V1`, `A2`, or none.
fn default_track_name(name: &str) -> bool {
    let name = name.trim();
    let numbered = |prefix: &str| {
        name.strip_prefix(prefix)
            .is_some_and(|n| !n.is_empty() && n.trim_start().bytes().all(|b| b.is_ascii_digit()))
    };
    name.is_empty() || ["Video", "Audio", "V", "A"].into_iter().any(numbered)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImportedMarker {
    pub at: i64,
    /// Length in frames, at least one.
    pub duration: i64,
    pub name: String,
    pub note: String,
    pub color: String,
    /// Its id in vidcut, when vidcut wrote the file.
    pub id: Option<String>,
}

/// What a project holds besides its timeline, as an OTIO file of the
/// whole timeline keeps it (`metadata.vidcut.project`). Other editors read
/// the timeline from the file's own items.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectMeta {
    pub settings: crate::model::Settings,
    /// The pool. In a bundle, paths are relative to its folder.
    pub media: Vec<Media>,
    pub bins: Vec<String>,
    pub tape_marks: crate::model::TapeMarks,
    pub next_id: crate::model::NextIds,
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
    pub transitions: Vec<ImportedTransition>,
    /// Settings of the tracks the clips go on.
    pub tracks: Vec<ImportedTrack>,
    pub markers: Vec<ImportedMarker>,
    pub marks: Marks,
    /// Whether the file has audio tracks. Without any, clips take their
    /// media's audio.
    pub has_audio_tracks: bool,
    /// The rest of the project, when vidcut wrote the file.
    pub project: Option<ProjectMeta>,
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

/// An absolute path on this system: `/…`, or on Windows a Windows one (a
/// server's written `//server/…` too).
fn is_local_absolute(s: &str) -> bool {
    if cfg!(windows) {
        windows_absolute(s) || s.starts_with("//")
    } else {
        s.starts_with('/')
    }
}

/// A path written with `/` (or on Windows either separator) as this system
/// writes it.
fn native(s: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(s.replace('/', "\\"))
    } else {
        PathBuf::from(s)
    }
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
                    _ if windows_absolute(r) => r,
                    _ => return None,
                },
            };
            let path = path
                .strip_prefix('/')
                .filter(|p| windows_absolute(p))
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
        let local = is_local_absolute(f);
        if !local && !absolute_elsewhere(f, cfg!(windows)) {
            // Relative to the timeline file.
            if let Some(b) = base {
                paths.push(b.join(native(f)));
                if !cfg!(windows) && f.contains('\\') {
                    paths.push(b.join(f.replace('\\', "/")));
                }
            }
            continue;
        }
        if local {
            paths.push(native(f));
        } else if cfg!(windows) {
            // A Unix path names that folder on the current drive.
            paths.push(PathBuf::from(f));
        }
        // Moved with the timeline file: its last parts next to the file.
        if let Some(b) = base {
            let parts: Vec<&str> = f
                .split(['/', '\\'])
                .filter(|s| !s.is_empty() && !is_drive(s))
                .collect();
            for k in (1..=parts.len().min(3)).rev() {
                let near = parts[parts.len() - k..]
                    .iter()
                    .fold(b.to_path_buf(), |p, s| p.join(s));
                paths.push(near);
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

/// A transition on a track, between the items with these indexes in
/// [`walk`]'s list.
struct Between<'a> {
    item: &'a Value,
    prev: Option<usize>,
    next: usize,
}

/// A track's items that take time, as `(start, length, item)`, and the
/// transitions between them (which overlap their neighbors instead).
fn walk<'a>(
    track: &'a Value,
    fps: Rational,
    warnings: &mut Vec<String>,
) -> (Vec<(i64, i64, &'a Value)>, Vec<Between<'a>>) {
    let mut out = Vec::new();
    let mut between = Vec::new();
    let mut t = 0;
    for item in track["children"].as_array().into_iter().flatten() {
        let s = schema(item);
        if s.starts_with("Transition") {
            between.push(Between {
                item,
                prev: out.len().checked_sub(1),
                next: out.len(),
            });
            continue;
        }
        if s.starts_with("Track") {
            warn(warnings, "nested tracks were left out (gaps instead)");
        } else if !s.starts_with("Gap") && !s.starts_with("Stack") && !is_clip(item) {
            warn(warnings, &format!("{s} items were left out (gaps instead)"));
        }
        let len = item_len(item, fps);
        out.push((t, len, item));
        t += len;
    }
    (out, between)
}

/// The transitions `between` items of a track, which became the clips with
/// `keys` (by item), as read.
fn transitions_of(
    between: Vec<Between>,
    keys: &[Option<usize>],
    sound: bool,
    fps: Rational,
    out: &mut Vec<ImportedTransition>,
) {
    for b in between {
        let key = |i: Option<usize>| i.and_then(|i| keys.get(i).copied().flatten());
        let (from, to) = (key(b.prev), key(Some(b.next)));
        if from.is_none() && to.is_none() {
            continue;
        }
        let offset = |k: &str| time(&b.item[k]).map_or(0, |(v, r)| frames(v, r, fps));
        out.push(ImportedTransition {
            sound,
            from,
            to,
            before: offset("in_offset"),
            after: offset("out_offset"),
            otio: b.item.clone(),
        });
    }
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

/// A clip's own Clip Color in a file vidcut wrote: `Some(None)` when the
/// clip shows its media's. `None` for other files.
fn kept_color(item: &Value) -> Option<Option<String>> {
    item["metadata"][VIDCUT]["clip"]
        .is_string()
        .then(|| our_str(item, "color"))
}

/// `doc`'s timeline as one compound clip named `name`: a timeline that
/// holds its tracks' stack on a video track for the picture and on an
/// audio track for the sound, as compound clips with sound are written.
/// The stack keeps its markers, which an import places on the timeline.
pub fn as_compound(doc: &Value, name: &str) -> Result<Value> {
    let tl = find_timeline(doc).ok_or_else(|| Error::usage("no Timeline in this OTIO file"))?;
    let tracks: Vec<&Value> = tl["tracks"]["children"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let fps = file_rate(tl, &tracks)?;
    let mut stack = tl["tracks"].clone();
    stack["name"] = json!(name);
    stack["source_range"] = range(0.0, item_len(&tl["tracks"], fps) as f64, fps.as_f64());
    let track = |kind: &str| {
        json!({
            "OTIO_SCHEMA": "Track.1",
            "kind": kind,
            "children": [stack.clone()],
        })
    };
    let children: Vec<Value> = ["Video", "Audio"]
        .into_iter()
        .filter(|kind| tracks.iter().any(|t| t["kind"] == *kind))
        .map(track)
        .collect();
    Ok(json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": name,
        "global_start_time": tl["global_start_time"],
        "tracks": { "OTIO_SCHEMA": "Stack.1", "children": children },
        "metadata": { VIDCUT: { "fps": fps } },
    }))
}

/// The timeline's rate: ours if we wrote the file, else the one most items
/// have.
fn file_rate(tl: &Value, tracks: &[&Value]) -> Result<Rational> {
    let ours = tl["metadata"][VIDCUT]["fps"]
        .as_str()
        .and_then(|s| s.parse::<Rational>().ok());
    match ours {
        Some(r) => Ok(r),
        None => rational(
            timeline_rate(tl, tracks)
                .ok_or_else(|| Error::usage("this OTIO file has no frame rate"))?,
        ),
    }
}

/// Whether `doc` is an OTIO file vidcut wrote of a whole timeline, which
/// holds the rest of its project too.
pub fn holds_project(doc: &Value) -> bool {
    doc["metadata"][VIDCUT]["project"].is_object()
}

/// A string vidcut keeps in `item`'s metadata.
fn our_str(item: &Value, key: &str) -> Option<String> {
    item["metadata"][VIDCUT][key].as_str().map(String::from)
}

/// A marker at timeline frame `at`.
fn marker(k: &Value, at: i64, fps: Rational) -> ImportedMarker {
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
    let duration = time(&k["marked_range"]["duration"]).map_or(1, |(v, r)| frames(v, r, fps));
    ImportedMarker {
        at,
        duration: duration.max(1),
        name: k["name"].as_str().unwrap_or("").to_string(),
        note: note.to_string(),
        color,
        id: our_str(k, "marker"),
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
            out.push(marker(k, start + offset, fps));
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
    let fps = file_rate(tl, &tracks)?;
    let start_tc = time(&tl["global_start_time"]).map_or(0, |(v, r)| frames(v, r, fps));
    let mut markers = Vec::new();

    // Video tracks become V1, V2…
    let mut clips: Vec<ImportedClip> = Vec::new();
    let mut found = Vec::new();
    let mut settings: Vec<ImportedTrack> = Vec::new();
    for (k, v) in video_tracks.iter().enumerate() {
        let id = format!("V{}", k + 1);
        settings.push(ImportedTrack::of(v, &id, false));
        let (items, between) = walk(v, fps, &mut warnings);
        let mut keys = vec![None; items.len()];
        for (i, (start, len, item)) in items.into_iter().enumerate() {
            let stack = schema(item).starts_with("Stack");
            if !(is_clip(item) || stack) || len <= 0 {
                continue;
            }
            let (effects, speed) = effects::read(item);
            let mut speed = speed_of(speed);
            let x = scalar(speed.as_ref());
            let reference = media_reference_of(item);
            if stack || schema(reference).starts_with("GeneratorReference") {
                clip_markers(item, start, len, fps, &mut markers);
                keys[i] = Some(clips.len());
                let mut c = ImportedClip::block(clips.len(), &id, item, (start, len), fps);
                c.effects = effects;
                c.speed = speed;
                if stack {
                    c.compound = Some(compound_of(item, fps));
                } else {
                    c.generator = Some(reference.clone());
                }
                clips.push(c);
                continue;
            }
            match clip_source(item, fps, base) {
                Ok((media, source_in, timecoded)) => {
                    let meta = &item["metadata"][VIDCUT];
                    let (first, skip) = first_shown(source_in, len, x);
                    if let Some(s) = speed.as_mut() {
                        s.skip = meta["skip"].as_i64().unwrap_or(skip);
                    }
                    clip_markers(item, start, len, fps, &mut markers);
                    keys[i] = Some(clips.len());
                    clips.push(ImportedClip {
                        key: clips.len(),
                        id: our_str(item, "clip"),
                        media_id: our_str(item, "media"),
                        track: id.clone(),
                        media,
                        name: item["name"].as_str().unwrap_or("").to_string(),
                        start,
                        duration: len,
                        // A still's, which its source range doesn't keep.
                        source_in: meta["source_in"].as_i64().unwrap_or(first),
                        timecoded: timecoded && meta["source_in"].is_null(),
                        video: true,
                        enabled: item["enabled"].as_bool().unwrap_or(true),
                        audio: None,
                        stream: meta["stream"].as_u64().unwrap_or(0) as u32,
                        gain_db: meta["gain_db"].as_f64().unwrap_or(0.0),
                        muted: meta["muted"].as_bool().unwrap_or(false),
                        link: link_group(item),
                        effects,
                        audio_effects: Vec::new(),
                        generator: None,
                        compound: None,
                        speed,
                        color: clip_color(item),
                        own_color: kept_color(item),
                    });
                }
                Err(e) => warnings.push(format!("{}; left as a gap", e.message)),
            }
        }
        transitions_of(between, &keys, false, fps, &mut found);
    }
    // An audio clip lining up with a video clip of the same media (on the
    // track vidcut named, else on any; in its link group if both
    // have one) is that clip's audio. Others are audio-only clips: on the
    // track vidcut named, else on audio tracks A1, A2… in the file's order.
    // The video clips of each media in order of start, and the longest:
    // those an audio clip overlaps are the last that start before it ends,
    // back to where the longest no longer reaches it.
    let mut pictures: HashMap<String, (Vec<usize>, i64)> = HashMap::new();
    for (i, c) in clips.iter().enumerate().filter(|(_, c)| c.video) {
        let (list, longest) = pictures.entry(c.media.url.clone()).or_default();
        list.push(i);
        *longest = (*longest).max(c.duration);
    }
    for (list, _) in pictures.values_mut() {
        list.sort_by_key(|&i| clips[i].start);
    }
    let mut foreign_audio = 0;
    for a in &audio_tracks {
        let hint = a["metadata"][VIDCUT]["track"].as_str().map(String::from);
        let mut own: Option<String> = None;
        let (items, between) = walk(a, fps, &mut warnings);
        let mut keys = vec![None; items.len()];
        for (i, (start, len, item)) in items.into_iter().enumerate() {
            let stack = schema(item).starts_with("Stack");
            if !(is_clip(item) || stack) || len <= 0 {
                continue;
            }
            let (effects, speed) = effects::read(item);
            let mut speed = speed_of(speed);
            let x = scalar(speed.as_ref());
            // A compound clip's sound: a block on an audio track.
            if stack {
                clip_markers(item, start, len, fps, &mut markers);
                let track = match &hint {
                    Some(h) if h.starts_with('A') => h.clone(),
                    _ => own
                        .get_or_insert_with(|| {
                            foreign_audio += 1;
                            format!("A{foreign_audio}")
                        })
                        .clone(),
                };
                keys[i] = Some(clips.len());
                let mut c = ImportedClip::block(clips.len(), &track, item, (start, len), fps);
                c.video = false;
                c.effects = effects;
                c.speed = speed;
                c.compound = Some(compound_of(item, fps));
                clips.push(c);
                continue;
            }
            let (media, source_in, timecoded) = match clip_source(item, fps, base) {
                Ok((media, start, timecoded)) => {
                    let meta = &item["metadata"][VIDCUT];
                    let (first, skip) = first_shown(start, len, x);
                    if let Some(s) = speed.as_mut() {
                        s.skip = meta["skip"].as_i64().unwrap_or(skip);
                    }
                    (
                        media,
                        meta["source_in"].as_i64().unwrap_or(first),
                        timecoded,
                    )
                }
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
            let gain = meta["gain_db"].as_f64().or_else(|| effects::gain(item));
            let stream = meta["stream"]
                .as_u64()
                .map(|s| s as u32)
                .or_else(|| editor_stream(item));
            let pairs = |c: &ImportedClip| {
                c.video
                    && c.audio.is_none()
                    && c.media.url == media.url
                    && c.timecoded == timecoded
                    // The same source frame at each time, at its speed.
                    && (c.source_in as f64 - x * c.start as f64 - (source_in as f64 - x * start as f64))
                        .abs()
                        < 1.0
                    && overlaps((c.start, c.end()), span)
                    && match (c.link, link) {
                        (Some(x), Some(y)) => x == y,
                        _ => true,
                    }
            };
            // The first of the file's clips that pairs.
            let owner = match &hint {
                Some(h) if h.starts_with('A') => None,
                _ => pictures.get(&media.url).and_then(|(list, longest)| {
                    let end = list.partition_point(|&i| clips[i].start < span.1);
                    list[..end]
                        .iter()
                        .rev()
                        .take_while(|&&i| clips[i].start + longest > span.0)
                        .copied()
                        .filter(|&i| {
                            hint.as_ref().is_none_or(|h| &clips[i].track == h) && pairs(&clips[i])
                        })
                        .min()
                }),
            };
            if let Some(c) = owner.map(|i| &mut clips[i]) {
                c.audio = Some((start - c.start, span.1 - c.end()));
                c.muted |= muted;
                if let Some(g) = gain {
                    c.gain_db = g;
                }
                if let Some(s) = stream {
                    c.stream = s;
                }
                c.audio_effects = effects;
                // The picture's, which editors write on both.
                if c.speed.is_none() {
                    c.speed = speed;
                }
                keys[i] = Some(c.key);
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
            keys[i] = Some(clips.len());
            clips.push(ImportedClip {
                key: clips.len(),
                id: our_str(item, "clip"),
                media_id: our_str(item, "media"),
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
                effects: Vec::new(),
                audio_effects: effects,
                generator: None,
                compound: None,
                speed,
                color: clip_color(item),
                own_color: kept_color(item),
            });
        }
        transitions_of(between, &keys, true, fps, &mut found);
        match (&hint, own) {
            (Some(h), _) if h.starts_with('A') => settings.push(ImportedTrack::of(a, h, true)),
            (None, Some(id)) => settings.push(ImportedTrack::of(a, &id, true)),
            _ => {}
        }
    }
    // Per track: clips that overlap what is already there lose their audio,
    // or go if that is all they have.
    // In order of track and start, the clips kept on a track are the last
    // ones kept: those that reach a time are the last that start before it,
    // back to where the longest kept on the track no longer reaches it.
    clips.sort_by(|x, y| x.track.cmp(&y.track).then(x.start.cmp(&y.start)));
    let (mut dropped, mut gone) = (0, 0);
    let mut kept: Vec<ImportedClip> = Vec::new();
    let (mut first, mut longest) = (0, 0);
    for mut c in clips {
        if kept.get(first).is_some_and(|k| k.track != c.track) {
            (first, longest) = (kept.len(), 0);
        }
        let reach = |k: &ImportedClip| k.end().max(k.audio_range().map_or(k.end(), |a| a.1));
        let near = |r: (i64, i64)| {
            kept[first..]
                .iter()
                .rev()
                .take_while(move |k| k.start + longest > r.0)
        };
        let span = (c.start, c.end());
        let busy = |r: (i64, i64)| {
            near(r).any(|k| {
                (k.video && overlaps((k.start, k.end()), r))
                    || k.audio_range().is_some_and(|a| overlaps(a, r))
                    || (!k.video && overlaps((k.start, k.end()), r))
            })
        };
        if !c.video && busy(span) {
            gone += 1;
            continue;
        }
        if c.video && near(span).any(|k| k.video && overlaps((k.start, k.end()), span)) {
            gone += 1;
            continue;
        }
        if let Some(r) = c.audio_range()
            && near(r).any(|k| k.audio_range().is_some_and(|a| overlaps(a, r)))
        {
            dropped += 1;
            c.audio = None;
            c.audio_effects.clear();
        }
        longest = longest.max(reach(&c) - c.start);
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
    let mut kept: Vec<String> = Vec::new();
    for e in clips
        .iter()
        .flat_map(|c| c.effects.iter().chain(&c.audio_effects))
    {
        let name = effects::name(e);
        if !kept.contains(&name) {
            kept.push(name);
        }
    }
    if !kept.is_empty() {
        warnings.push(format!(
            "clip effects kept on their clips but not shown or played: {}",
            kept.join(", ")
        ));
    }
    let mut made: Vec<&str> = Vec::new();
    for c in clips.iter().filter(|c| c.generator.is_some()) {
        if !made.contains(&c.name.as_str()) {
            made.push(&c.name);
        }
    }
    if !made.is_empty() {
        warnings.push(format!(
            "generators and titles kept as blocks but played as gaps: {}",
            made.join(", ")
        ));
    }
    // Time effects without a speed play at normal speed.
    let mut speeds: Vec<String> = Vec::new();
    for speed in clips
        .iter()
        .filter_map(|c| c.speed.as_ref())
        .filter(|s| s.scalar().is_none())
    {
        let label = speed.label();
        if !speeds.contains(&label) {
            speeds.push(label);
        }
    }
    if !speeds.is_empty() {
        warnings.push(format!(
            "time effects kept on their clips but played at normal speed: {}",
            speeds.join(", ")
        ));
    }
    let mut nested: Vec<&str> = Vec::new();
    for c in clips.iter().filter(|c| c.compound.is_some()) {
        if !nested.contains(&c.name.as_str()) {
            nested.push(&c.name);
        }
    }
    if !nested.is_empty() {
        warnings.push(format!("{COMPOUNDS_KEPT}: {}", nested.join(", ")));
    }
    // Transitions with a clip still on a side.
    let have: std::collections::HashSet<usize> = clips.iter().map(|c| c.key).collect();
    found.retain(|t| {
        [t.from, t.to]
            .into_iter()
            .flatten()
            .any(|k| have.contains(&k))
    });
    let mut names: Vec<String> = Vec::new();
    for t in &found {
        let name = transitions::otio_name(&t.otio);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if !names.is_empty() {
        warnings.push(format!(
            "transitions kept on their edit points but shown as cuts: {}",
            names.join(", ")
        ));
    }

    // Markers of the timeline and of its tracks (tracks start with it).
    for list in
        std::iter::once(&tl["tracks"]["markers"]).chain(tracks.iter().map(|t| &t["markers"]))
    {
        for k in list.as_array().into_iter().flatten() {
            if let Some((v, r)) = time(&k["marked_range"]["start_time"]) {
                markers.push(marker(k, frames(v, r, fps), fps));
            }
        }
    }
    markers.retain(|k| k.at >= 0);
    // A clip's video and audio may carry the same markers.
    markers.sort();
    markers.dedup();
    let marks: Marks =
        serde_json::from_value(tl["metadata"][VIDCUT]["marks"].clone()).unwrap_or_default();
    let project = tl["metadata"][VIDCUT]
        .get("project")
        .and_then(|v| match serde_json::from_value::<ProjectMeta>(v.clone()) {
            Ok(meta) => Some(meta),
            Err(e) => {
                warnings.push(format!(
                    "the vidcut project in the file couldn't be read ({e}): only its timeline was"
                ));
                None
            }
        })
        .map(|mut meta| {
            // Relative to the file's folder, as in a bundle.
            if let Some(base) = base {
                for m in &mut meta.media {
                    if m.path.is_relative() {
                        m.path = base.join(native(&m.path.to_string_lossy()));
                    }
                }
            }
            meta
        });
    Ok(Imported {
        name: tl["name"].as_str().unwrap_or("").to_string(),
        fps,
        start_tc,
        clips,
        transitions: found,
        tracks: settings,
        markers,
        marks,
        has_audio_tracks: !audio_tracks.is_empty(),
        project,
        warnings,
    })
}
