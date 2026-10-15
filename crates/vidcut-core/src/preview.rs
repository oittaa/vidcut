//! What the viewer plays of a render plan (docs/decisions/preview.md): the
//! plan itself, the export's, with fresh proxies' pictures in place of
//! their media's where the project plays proxies (decision D41).

use std::path::Path;

use crate::model::Project;
use crate::render::{RenderPlan, VideoPiece};

/// Plays fresh proxies in `plan` when the project asks for them (decision
/// D41). Video paths become the proxy file and image sequences play as
/// that video. `origin` and `source_frame` stay: proxy frames keep their
/// source's timestamps, so the same frames are picked. Audio paths stay on
/// the originals. `exists` is whether the proxy file is there: a missing
/// file plays the original. Export plans never call this.
pub fn apply_proxies(plan: &mut RenderPlan, project: &Project, exists: impl Fn(&Path) -> bool) {
    if !project.settings.use_proxies {
        return;
    }
    for piece in plan.video.iter_mut().flat_map(VideoPiece::layers_mut) {
        let VideoPiece::Source {
            media, path, image, ..
        } = piece
        else {
            continue;
        };
        let Some(proxy) = project.media(media).and_then(|m| m.fresh_proxy()) else {
            continue;
        };
        if exists(&proxy.path) {
            *path = proxy.path.clone();
            *image = None;
        }
    }
}
