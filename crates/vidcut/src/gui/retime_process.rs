use serde_json::json;
use vidcut_core::model::{ClipRetime, ProjectRetime, Retime};

use super::workspace::Workspace;

#[cfg(target_os = "macos")]
pub(super) const PROCESSES: [ClipRetime; 4] = [
    ClipRetime::Project,
    ClipRetime::Nearest,
    ClipRetime::FrameBlend,
    ClipRetime::Macos,
];

#[cfg(not(target_os = "macos"))]
pub(super) const PROCESSES: [ClipRetime; 5] = [
    ClipRetime::Project,
    ClipRetime::Nearest,
    ClipRetime::FrameBlend,
    ClipRetime::AiFast,
    ClipRetime::AiBest,
];

#[cfg(target_os = "macos")]
pub(super) const PROJECT_PROCESSES: [ProjectRetime; 4] = [
    ProjectRetime::Auto,
    ProjectRetime::Nearest,
    ProjectRetime::FrameBlend,
    ProjectRetime::Macos,
];

#[cfg(not(target_os = "macos"))]
pub(super) const PROJECT_PROCESSES: [ProjectRetime; 5] = [
    ProjectRetime::Auto,
    ProjectRetime::Nearest,
    ProjectRetime::FrameBlend,
    ProjectRetime::AiFast,
    ProjectRetime::AiBest,
];

pub(super) fn clip_text(r: ClipRetime, project: Retime) -> String {
    match r {
        ClipRetime::Project => format!("Project Settings ({})", project.title()),
        r => r.title().into(),
    }
}

pub(super) fn project_text(r: ProjectRetime) -> String {
    match r {
        ProjectRetime::Auto => format!("Auto ({} for slow motion)", Retime::ai_here().title()),
        r => r.title().into(),
    }
}

impl Workspace {
    pub(super) fn set_retime(&mut self, clips: &[String], retime: ClipRetime) {
        self.send_asking("clip.set", json!({ "clips": clips, "retime": retime }));
    }
}
