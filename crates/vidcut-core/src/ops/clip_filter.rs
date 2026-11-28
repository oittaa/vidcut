use crate::error::{Error, Result};
use crate::model::{Clip, ClipFilter, PictureTool, Project, Tool, Track, TrackKind};

pub fn check(f: &ClipFilter) -> Result<()> {
    match f {
        ClipFilter::ClipColor { color: Some(c) } => super::check_clip_color(c),
        ClipFilter::Marked { color: Some(c) } => super::check_color(c),
        _ => Ok(()),
    }
}

pub fn strip<'a>(p: &'a Project, f: Option<&ClipFilter>) -> Vec<(&'a Track, &'a Clip)> {
    let mut clips: Vec<_> = (p.tracks.iter())
        .filter(|t| t.kind == TrackKind::Video && t.video_enabled)
        .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
        .filter(|(_, c)| !c.is_generator() && f.is_none_or(|f| admits(p, c, f)))
        .collect();
    clips.sort_by_key(|(t, c)| (c.start, c.end(), t.number()));
    clips
}

pub fn met(p: &Project, f: &ClipFilter, (a, b): (i64, i64)) -> Result<Vec<String>> {
    let ids: Vec<String> = (strip(p, Some(f)).into_iter())
        .filter(|(_, c)| c.start < b && c.end() > a)
        .map(|(_, c)| c.id.clone())
        .collect();
    match ids.is_empty() {
        true => Err(Error::invalid("no clip in the range meets the clip filter")),
        false => Ok(ids),
    }
}

pub fn span(p: &Project, f: &ClipFilter) -> Result<(i64, i64)> {
    if f.is_all() {
        return Err(Error::usage("render filtered needs a clip filter"));
    }
    let clips = strip(p, Some(f));
    let a = clips.iter().map(|(_, c)| c.start).min();
    let b = clips.iter().map(|(_, c)| c.end()).max();
    a.zip(b)
        .ok_or_else(|| Error::invalid("no clip meets the clip filter"))
}

pub fn admits(p: &Project, c: &Clip, f: &ClipFilter) -> bool {
    let picture = c.picture.as_ref();
    let media = p.media(&c.media);
    let graded = || (picture.and_then(|x| x.grade.as_ref())).is_some_and(|g| g.is_active());
    let marked = |color: Option<&str>| {
        let Some(m) = media else {
            return false;
        };
        let (lo, hi) = c.shown();
        (m.markers.iter())
            .filter(|k| k.at < hi && k.at + k.duration.max(1) > lo)
            .any(|k| color.is_none_or(|x| k.color == x))
    };
    let sizing = |x: &crate::model::Picture| {
        [PictureTool::Transform, PictureTool::Crop]
            .into_iter()
            .any(|tool| super::picture::cleared(x, tool) != *x)
    };
    match f {
        ClipFilter::All => true,
        ClipFilter::Graded => graded(),
        ClipFilter::Ungraded => !graded(),
        ClipFilter::Composition => {
            (picture.and_then(|x| x.composition.as_ref())).is_some_and(|k| {
                (k.nodes.iter()).any(|n| !matches!(n.tool, Tool::MediaIn | Tool::MediaOut))
            })
        }
        ClipFilter::NoClipColor => c.shown_color(media).is_none(),
        ClipFilter::ClipColor { color } => {
            (c.shown_color(media)).is_some_and(|x| color.as_deref().is_none_or(|want| x == want))
        }
        ClipFilter::NoMarker => !marked(None),
        ClipFilter::Marked { color } => marked(color.as_deref()),
        ClipFilter::Effects => picture.is_some_and(|x| !x.effects.is_empty()),
        ClipFilter::OtherRate => media.is_some_and(|m| m.rate(p.fps()) != p.fps()),
        ClipFilter::Sizing => picture.is_some_and(|p| sizing(p)),
    }
}

pub fn top_at(clips: &[(&Track, &Clip)], at: i64) -> Option<usize> {
    (clips.iter().enumerate())
        .filter(|(_, (_, c))| c.start <= at && at < c.end())
        .max_by_key(|(_, (t, _))| t.number())
        .map(|(i, _)| i)
}

pub fn prior(p: &Project, at: i64, back: u32) -> Result<(String, String)> {
    let clips = strip(p, p.timeline().map(|t| &t.clip_filter));
    let i = top_at(&clips, at)
        .ok_or_else(|| Error::not_found("no clip of the Thumbnail timeline is at the playhead"))?;
    let current = clips[i].1.id.clone();
    let from = (i.checked_sub(back as usize)).ok_or_else(|| {
        Error::not_found(format!(
            "{current} has no clip {back} places before it in the Thumbnail timeline"
        ))
    })?;
    Ok((clips[from].1.id.clone(), current))
}
