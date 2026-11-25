use crate::analysis::Found;
use crate::error::{Error, Result};
use crate::model::{Clip, Picture, Project, Subject, TrackKind};

use super::{Placed, place, track_above, tracks, unlocked};

const SECONDS: f64 = 5.0;
const NO_FACE_ZOOM: f64 = 2.0;
const ROOM: f64 = 1.1;
pub const FACE_ZOOM: f64 = 1.3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Framing {
    pub zoom: f64,
    pub position: [f64; 2],
}

pub fn target(p: &Project, playhead: i64) -> Result<Option<Clip>> {
    let (mark_in, mark_out) = (p.marks.mark_in, p.marks.mark_out);
    let at = mark_in.or(mark_out.map(|o| o - 1)).unwrap_or(playhead);
    let found = (p.tracks_of(TrackKind::Video).into_iter().rev())
        .filter(|&i| p.tracks[i].video_enabled)
        .find_map(|i| {
            let ci = p.tracks[i].clip_at(at)?;
            let c = &p.tracks[i].clips[ci];
            (c.enabled && c.video).then_some((i, ci))
        });
    let Some((ti, ci)) = found else {
        return Ok(None);
    };
    let mut clip = p.tracks[ti].clips[ci].clone();
    let (start, end) = match (mark_in, mark_out) {
        (Some(a), Some(b)) => (a, b),
        (Some(a), None) => (a, clip.end()),
        (None, Some(b)) => (clip.start, b),
        (None, None) => (playhead, playhead + p.fps().secs_to_frames(SECONDS)),
    };
    let (start, end) = (start.max(clip.start), end.min(clip.end()));
    if start >= end {
        return Err(Error::invalid("the marks hold no frame of the clip"));
    }
    clip.move_head(start - clip.start);
    clip.duration = end - start;
    Ok(Some(clip))
}

pub fn framing(faces: &[Found], fit: [f64; 4], [w, h]: [f64; 2], zoom: f64) -> Framing {
    let least = f64::from(crate::motion::Join::of(Subject::Face).start);
    let [fx, fy, fw, fh] = fit;
    let boxes: Vec<[f64; 4]> = faces
        .iter()
        .filter(|f| f64::from(f.score) >= least)
        .map(|f| {
            let ([u, v], [a, b]) = (f.center.map(f64::from), f.size.map(f64::from));
            [u - a / 2.0, v - b / 2.0, u + a / 2.0, v + b / 2.0]
        })
        .filter(|&[x0, y0, x1, y1]| x0 >= 0.0 && y0 >= 0.0 && x1 <= 1.0 && y1 <= 1.0)
        .map(|[x0, y0, x1, y1]| {
            [
                (fx + x0 * fw) / w,
                (fy + y0 * fh) / h,
                (fx + x1 * fw) / w,
                (fy + y1 * fh) / h,
            ]
        })
        .collect();
    if boxes.is_empty() {
        return Framing {
            zoom: NO_FACE_ZOOM,
            position: [0.5; 2],
        };
    }
    let low = |i: usize| boxes.iter().map(|b| b[i]).fold(f64::INFINITY, f64::min);
    let high = |i: usize| boxes.iter().map(|b| b[i]).fold(f64::NEG_INFINITY, f64::max);
    let ([x0, y0], [x1, y1]) = ([low(0), low(1)], [high(2), high(3)]);
    if (x1 - x0).max(y1 - y0) * ROOM * zoom > 1.0 {
        return Framing {
            zoom: 1.0,
            position: [0.5; 2],
        };
    }
    let limit = (zoom - 1.0) / 2.0;
    let place = |lo: f64, hi: f64| 0.5 + (zoom * (0.5 - (lo + hi) / 2.0)).clamp(-limit, limit);
    Framing {
        zoom,
        position: [place(x0, x1), place(y0, y1)],
    }
}

pub fn close_up(p: &mut Project, mut clip: Clip, framing: Framing) -> Result<Placed> {
    let reference = std::mem::replace(&mut clip.id, p.alloc_clip_id());
    let (start, end) = (clip.start, clip.end());
    clip.audio = None;
    clip.effects = None;
    clip.picture = Some(Picture {
        zoom: Some([framing.zoom; 2]),
        position: (framing.position != [0.5; 2]).then_some(framing.position),
        ..Picture::default()
    });
    let video = p.tracks_of(TrackKind::Video).len();
    let to = track_above(p, start, end);
    for _ in video..p.tracks_of(TrackKind::Video).len() {
        tracks::add_track(p, TrackKind::Audio);
    }
    unlocked(p, to)?;
    let (id, track) = (clip.id.clone(), p.tracks[to].id.clone());
    place(p, to, clip);
    p.marks = Default::default();
    Ok(Placed {
        clip: id,
        track,
        reference,
        playhead: end,
    })
}
