use std::path::Path;

use crate::model::{ColorSpace, Media, Project, SuperScale};
use crate::render::{RenderPlan, VideoPiece};

pub fn apply_proxies(
    plan: &mut RenderPlan,
    project: &Project,
    proxies: &Path,
    exists: impl Fn(&Path) -> bool,
    unfit: impl Fn(&Media, SuperScale) -> bool,
) {
    if !project.settings.use_proxies {
        return;
    }
    for piece in plan.video.iter_mut().flat_map(VideoPiece::layers_mut) {
        let VideoPiece::Source {
            media,
            path,
            image,
            color_space,
            super_scale,
            ..
        } = piece
        else {
            continue;
        };
        let Some(m) = project.media(media).filter(|m| !unfit(m, *super_scale)) else {
            continue;
        };
        let Some(proxy) = m.fresh_proxy() else {
            continue;
        };
        let file = proxy.path_in(proxies);
        if exists(&file) {
            crate::disk_cache::used(&file);
            *path = file;
            *image = None;
            *color_space = ColorSpace::Tagged;
        }
    }
}
