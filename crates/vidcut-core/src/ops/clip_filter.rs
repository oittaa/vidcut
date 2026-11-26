use crate::error::Result;
use crate::model::{Clip, ClipFilter, PictureTool, Project, Tool};

pub fn check(f: &ClipFilter) -> Result<()> {
    match f {
        ClipFilter::ClipColor { color: Some(c) } => super::check_clip_color(c),
        ClipFilter::Marked { color: Some(c) } => super::check_color(c),
        _ => Ok(()),
    }
}

pub fn admits(p: &Project, c: &Clip, f: &ClipFilter) -> bool {
    let picture = c.picture.as_ref();
    let media = p.media(&c.media);
    let graded = || {
        let grade = picture.and_then(|x| x.grade.as_ref());
        grade.is_some_and(|g| !g.is_usual(p.frame_size()))
    };
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
        ClipFilter::Sizing => picture.is_some_and(sizing),
    }
}
