use crate::error::{Error, Result};
use crate::model::{Picture, Project, TrackKind};

use super::{Placed, place, track_above, tracks, unlocked};

const SECONDS: f64 = 5.0;
const ZOOM: f64 = 2.0;

pub fn close_up(p: &mut Project, playhead: i64) -> Result<Option<Placed>> {
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
    let reference = std::mem::replace(&mut clip.id, p.alloc_clip_id());
    clip.move_head(start - clip.start);
    clip.duration = end - start;
    clip.audio = None;
    clip.effects = None;
    clip.picture = Some(Picture {
        zoom: Some([ZOOM; 2]),
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
    Ok(Some(Placed {
        clip: id,
        track,
        reference,
        playhead: end,
    }))
}
