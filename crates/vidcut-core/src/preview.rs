use std::path::Path;

use crate::model::Project;
use crate::render::{RenderPlan, VideoPiece};

pub fn apply_proxies(
    plan: &mut RenderPlan,
    project: &Project,
    proxies: &Path,
    exists: impl Fn(&Path) -> bool,
) {
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
        let file = proxy.path_in(proxies);
        if exists(&file) {
            *path = file;
            *image = None;
        }
    }
}
