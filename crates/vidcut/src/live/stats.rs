//! Stats for nerds: what the viewer's player does, gathered from the
//! engine, the picture shown and the renderer. `player.state` returns them
//! as `stats`; the viewer's panel shows the same as lines.

use serde::{Deserialize, Serialize};
use vidcut_av::player::{self as engine, PlaysOn};

/// What the renderer tells of the viewer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewerStats {
    /// The size it draws the picture at, in physical pixels.
    pub size: [u32; 2],
    /// How the picture got on screen: `uploaded`, `shown from GPU memory`,
    /// `copied down and uploaded`.
    pub shown: String,
    /// The GPU it draws with: wgpu's backend, the adapter and its driver.
    pub renderer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerStats {
    /// The viewer, when a renderer tells of it.
    pub viewer: Option<ViewerStats>,
    /// Frames shown exactly; while playing, presents late and frames
    /// passed over.
    pub frames: u64,
    pub late: u64,
    pub skipped: u64,
    /// The picture shown last; `None` before the first.
    pub picture: Option<PictureStats>,
    /// What the file of the picture holds and what decoded it.
    pub decoding: Option<String>,
    /// Why the player decodes on the CPU, when it chose so.
    pub cpu: Option<String>,
    pub timeline: TimelineStats,
    pub kept: KeptStats,
    pub sound: SoundStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PictureStats {
    /// The frame the viewer is at, and whether the picture is that frame's
    /// (false: an earlier one, while it decodes).
    pub frame: i64,
    pub exact: bool,
    pub size: [u32; 2],
    pub layout: String,
    pub colors: String,
    /// Where its pixels are: `memory`, or a GPU's (`NVDEC memory`).
    pub memory: String,
    /// What decoded it: `cpu`, `nvdec`, …; `None` for black.
    pub decoder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineStats {
    pub size: [u32; 2],
    pub fps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeptStats {
    /// Pictures kept right after the frame the viewer is at and right
    /// before it (a minute each way at most), and all of them.
    pub ahead: usize,
    pub behind: usize,
    pub pictures: usize,
    /// MB kept in memory and in video memory, each with its budget.
    pub memory_mb: [usize; 2],
    pub video_mb: [usize; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundStats {
    /// The device: the id of the one chosen, else the system's default.
    pub device: String,
    pub rate: u32,
    pub channels: u16,
    pub latency_ms: Option<f64>,
}

impl PlayerStats {
    pub fn of(engine: &engine::Player, viewer: Option<ViewerStats>) -> Self {
        let s = engine.stats();
        let mb = |(used, budget): (usize, usize)| [used >> 20, budget >> 20];
        let picture = s.shown.as_ref().map(|shown| {
            let p = &shown.picture;
            let c = p.colors();
            let range = if c.full_range { "full" } else { "video" };
            let hdr = c.hdr.map_or(String::new(), |h| format!(", {h:?}"));
            PictureStats {
                frame: shown.frame,
                exact: shown.exact,
                size: [p.width(), p.height()],
                layout: format!("{:?}", p.layout()),
                colors: format!("{:?} {range} range{hdr}", c.matrix),
                memory: p
                    .gpu()
                    .map_or("memory".into(), |k| format!("{} memory", k.label())),
                decoder: p.decoder().map(|d| d.name().to_string()),
            }
        });
        let device = match &s.plays_on {
            PlaysOn::Default => "the system's default".into(),
            PlaysOn::Chosen(id) => id.clone(),
            PlaysOn::Missing(id) => format!("the system's default ({id} isn't connected)"),
        };
        Self {
            viewer,
            frames: s.frames,
            late: s.late,
            skipped: s.skipped,
            decoding: s
                .shown
                .as_ref()
                .and_then(|shown| shown.picture.decoding().map(String::from)),
            picture,
            cpu: s.cpu.map(String::from),
            timeline: TimelineStats {
                size: [s.size.0, s.size.1],
                fps: s.fps.as_f64(),
            },
            kept: KeptStats {
                ahead: s.ahead,
                behind: s.behind,
                pictures: s.kept,
                memory_mb: mb(s.memory),
                video_mb: mb(s.video),
            },
            sound: SoundStats {
                device,
                rate: s.rate,
                channels: s.channels,
                latency_ms: s.latency.map(|l| (l.as_secs_f64() * 1000.0).round()),
            },
        }
    }

    /// What the viewer's panel shows: a name and its facts on each line.
    pub fn lines(&self) -> Vec<(&'static str, String)> {
        let mut lines = Vec::new();
        let counts = format!(
            "{} frames shown, {} late, {} skipped",
            self.frames, self.late, self.skipped
        );
        lines.push((
            "Viewer",
            match &self.viewer {
                Some(v) => format!("{}×{}: {counts}", v.size[0], v.size[1]),
                None => counts,
            },
        ));
        if let Some(p) = &self.picture {
            let shown = self
                .viewer
                .as_ref()
                .map_or(String::new(), |v| format!(", {}", v.shown));
            let late = if p.exact {
                ""
            } else {
                " (the picture before it)"
            };
            lines.push((
                "Picture",
                format!(
                    "frame {}{late}: {}×{} {}, {}, in {}{shown}",
                    p.frame, p.size[0], p.size[1], p.layout, p.colors, p.memory
                ),
            ));
        }
        if let Some(d) = &self.decoding {
            let why = self
                .cpu
                .as_ref()
                .map_or(String::new(), |w| format!(" ({w})"));
            lines.push(("Decoding", format!("{d}{why}")));
        }
        let t = &self.timeline;
        lines.push((
            "Timeline",
            format!("{}×{} at {} fps", t.size[0], t.size[1], fps(t.fps)),
        ));
        let k = &self.kept;
        lines.push((
            "Kept",
            format!(
                "{} ahead, {} behind, {} pictures: {} of {} MB of memory, {} of {} MB of video memory",
                k.ahead,
                k.behind,
                k.pictures,
                k.memory_mb[0],
                k.memory_mb[1],
                k.video_mb[0],
                k.video_mb[1]
            ),
        ));
        let s = &self.sound;
        let latency = s
            .latency_ms
            .map_or(String::new(), |l| format!(", {l:.0} ms latency"));
        lines.push((
            "Sound",
            format!(
                "{}, {} Hz, {} channels{latency}",
                s.device, s.rate, s.channels
            ),
        ));
        if let Some(v) = &self.viewer {
            lines.push(("Renderer", v.renderer.clone()));
        }
        lines
    }
}

/// A rate as people write it: `30`, `29.97`.
fn fps(rate: f64) -> String {
    let text = format!("{rate:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}
