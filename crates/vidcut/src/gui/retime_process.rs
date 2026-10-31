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
    ClipRetime::AiSlow,
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
    ProjectRetime::AiSlow,
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

pub(super) fn choices() -> Vec<(&'static str, &'static str)> {
    PROCESSES
        .iter()
        .filter_map(|c| c.process().map(|r| (r.title(), does(r))))
        .collect()
}

fn does(r: Retime) -> &'static str {
    match r {
        Retime::Nearest => "Repeats or skips source frames. Makes no new frames.",
        Retime::FrameBlend => "Mixes the two source frames around each frame.",
        Retime::AiFast => "Makes new frames from the motion. Fast, also for UHD video.",
        Retime::AiSlow => "Slower than AI Fast, with frames nearer to the real ones.",
        Retime::Macos => "The frame rate conversion of macOS. Makes new frames from the motion.",
    }
}

impl Workspace {
    pub(super) fn set_retime(&mut self, clips: &[String], retime: ClipRetime) {
        self.send_asking("clip.set", json!({ "clips": clips, "retime": retime }));
    }
}
