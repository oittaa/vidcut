//! Stats for nerds: what the viewer's player does, gathered from the
//! engine, the picture shown and the renderer. `player.state` returns them
//! as `stats`. The viewer's panel and `player stats` show them in groups,
//! a fact on each row.

use serde::{Deserialize, Serialize};
use vidcut_av::color::Matrix;
use vidcut_av::player::{self as engine, Layout, PlaysOn};
use vidcut_core::model::Hdr;

/// A heading and its rows, a label and a fact on each.
pub type Group = (&'static str, Vec<(&'static str, String)>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerStats {
    /// What the file of the picture shown holds and what decoded it.
    /// `None` before the first picture, and for black.
    pub decoding: Option<DecodingStats>,
    /// The picture shown last. `None` before the first.
    pub picture: Option<PictureStats>,
    /// The viewer, when a renderer tells of it.
    pub viewer: Option<ViewerStats>,
    pub playback: PlaybackStats,
    pub kept: KeptStats,
    pub sound: SoundStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodingStats {
    /// FFmpeg's names of the codec and its profile: `h264`, `High`.
    pub codec: String,
    pub profile: Option<String>,
    /// The file's frame rate, when it gives one.
    pub fps: Option<f64>,
    pub interlaced: bool,
    /// The clockwise turn that shows it upright: 0, 90, 180 or 270.
    pub rotation: u16,
    /// FFmpeg's decoder: `h264`, `hevc`.
    pub decoder: String,
    /// `CPU`, or the GPU decoder: `NVDEC on NVIDIA GeForce RTX 4070`.
    pub runs_on: String,
    /// Why it runs on the CPU: GPU decoding is off, there is no GPU
    /// decoder, or the GPU's decoder wouldn't take the file.
    pub why_cpu: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PictureStats {
    /// The frame the viewer is at, and whether the picture is that frame's
    /// (false: an earlier one, while it decodes).
    pub frame: i64,
    pub exact: bool,
    pub size: [u32; 2],
    /// `YUV420P`, `NV12`, `P010`, `YUV420P10`, `YUV422P`, `YUV422P10`,
    /// `P210`, `YUV444P12` or `P416`.
    pub layout: String,
    /// `BT.601`, `BT.709` or `BT.2020`.
    pub matrix: String,
    /// `video` or `full`.
    pub range: String,
    /// `PQ` or `HLG`. `None` for SDR.
    pub hdr: Option<String>,
    /// Where its pixels are: `memory`, or a GPU's (`NVDEC memory`).
    pub memory: String,
}

/// The viewer's two-up (the Trim Editor's and Dynamic Trim's): what it
/// shows, and its frames since the GUI started.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TwoUpStats {
    /// `None` while the viewer shows the player.
    pub shown: Option<TwoUpShown>,
    /// Frames its cells were asked for, and those they never showed:
    /// another was asked for before the frame's picture was there.
    pub asked: u64,
    pub passed_over: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TwoUpShown {
    /// `trim_editor` or `dynamic_trim`.
    pub mode: String,
    /// The left cell and the right; `None` where no clip is.
    pub cells: [Option<TwoUpCell>; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TwoUpCell {
    /// The timeline frame it shows, and the source frame of its clip there.
    pub frame: i64,
    pub source_frame: i64,
    /// The picture is that frame's: `false` while it decodes, the cell
    /// showing the one before.
    pub exact: bool,
}

/// What the renderer tells of the viewer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewerStats {
    /// The size it draws the picture at, in physical pixels.
    pub size: [u32; 2],
    /// How the picture got on screen: `uploaded`, `shown from GPU memory`,
    /// `copied down and uploaded`. `None` before the first picture.
    pub shown: Option<String>,
    /// What draws it: wgpu's backend (`Vulkan`), the GPU and its driver.
    pub backend: String,
    pub gpu: String,
    pub driver: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackStats {
    /// Frames shown exactly. While playing, frames late and frames passed
    /// over.
    pub frames: u64,
    pub late: u64,
    pub skipped: u64,
    /// Frames copied from GPU memory down to memory, and from memory up to
    /// the GPU, since the player started: by its decoder and the viewer. A
    /// switch of decoder keeps counting.
    pub copied_down: CopiedStats,
    pub copied_up: CopiedStats,
}

/// Frames copied one way, and their bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopiedStats {
    pub frames: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeptStats {
    /// Pictures kept right after the frame the viewer is at and right
    /// before it (a minute each way at most), and all of them.
    pub ahead: usize,
    pub behind: usize,
    pub pictures: usize,
    /// MB kept in memory and in video memory, each with the budget it
    /// counts in.
    pub memory_mb: [usize; 2],
    pub video_mb: [usize; 2],
    /// Both count in the memory budget: the GPU's memory is the computer's.
    pub shared: bool,
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
        let decoding = s
            .shown
            .as_ref()
            .and_then(|shown| shown.picture.decoding())
            .map(|d| DecodingStats {
                codec: d.codec.clone(),
                profile: d.profile.clone(),
                fps: d.fps,
                interlaced: d.interlaced,
                rotation: d.rotation,
                decoder: d.decoder.clone(),
                runs_on: d.gpu.clone().unwrap_or_else(|| "CPU".into()),
                why_cpu: d
                    .why
                    .clone()
                    .or(s.cpu.map(String::from))
                    .filter(|_| d.gpu.is_none()),
            });
        let picture = s.shown.as_ref().map(|shown| {
            let p = &shown.picture;
            let c = p.colors();
            let layout = match p.layout() {
                Layout::Yuv420p => "YUV420P",
                Layout::Nv12 => "NV12",
                Layout::P010 => "P010",
                Layout::Yuv420p10 => "YUV420P10",
                Layout::Yuv422p => "YUV422P",
                Layout::Yuv422p10 => "YUV422P10",
                Layout::P210 => "P210",
                Layout::Yuv444p12 => "YUV444P12",
                Layout::P416 => "P416",
            };
            let matrix = match c.matrix {
                Matrix::Bt601 => "BT.601",
                Matrix::Bt709 => "BT.709",
                Matrix::Bt2020 => "BT.2020",
            };
            PictureStats {
                frame: shown.frame,
                exact: shown.exact,
                size: [p.width(), p.height()],
                layout: layout.into(),
                matrix: matrix.into(),
                range: if c.full_range { "full" } else { "video" }.into(),
                hdr: c.hdr.map(|h| {
                    match h {
                        Hdr::Pq => "PQ",
                        Hdr::Hlg => "HLG",
                    }
                    .into()
                }),
                memory: p
                    .gpu()
                    .map_or("memory".into(), |k| format!("{} memory", k.label())),
            }
        });
        let device = match &s.plays_on {
            PlaysOn::Default => "System default".into(),
            PlaysOn::Chosen(id) => id.clone(),
            PlaysOn::Missing(id) => format!("System default ({id} isn't connected)"),
        };
        Self {
            decoding,
            picture,
            viewer,
            playback: PlaybackStats {
                frames: s.frames,
                late: s.late,
                skipped: s.skipped,
                copied_down: CopiedStats {
                    frames: s.copies.down,
                    bytes: s.copies.down_bytes,
                },
                copied_up: CopiedStats {
                    frames: s.copies.up,
                    bytes: s.copies.up_bytes,
                },
            },
            kept: KeptStats {
                ahead: s.ahead,
                behind: s.behind,
                pictures: s.kept,
                memory_mb: mb(s.memory),
                video_mb: mb(s.video),
                shared: s.shared,
            },
            sound: SoundStats {
                device,
                rate: s.rate,
                channels: s.channels,
                latency_ms: s.latency.map(|l| (l.as_secs_f64() * 1000.0).round()),
            },
        }
    }

    /// What the viewer's panel shows: a group for each part of the player,
    /// in the order the picture goes through them. Facts that don't apply
    /// are left out.
    pub fn groups(&self) -> Vec<Group> {
        let mut groups = Vec::new();
        if let Some(d) = &self.decoding {
            groups.push(("Decoding", d.rows()));
        }
        if let Some(p) = &self.picture {
            groups.push(("Picture", p.rows()));
        }
        if let Some(v) = &self.viewer {
            groups.push(("Viewer", v.rows()));
        }
        groups.push(("Playback", self.playback.rows()));
        groups.push(("Kept", self.kept.rows()));
        groups.push(("Sound", self.sound.rows()));
        groups
    }
}

impl DecodingStats {
    fn rows(&self) -> Vec<(&'static str, String)> {
        let codec = match &self.profile {
            Some(p) => format!("{} {p}", self.codec),
            None => self.codec.clone(),
        };
        let mut rows = vec![("Codec", codec)];
        if let Some(rate) = self.fps {
            rows.push(("Frame rate", format!("{} fps", fps(rate))));
        }
        if self.interlaced {
            rows.push(("Interlaced", "yes".into()));
        }
        if self.rotation != 0 {
            rows.push(("Turned", format!("{}°", self.rotation)));
        }
        rows.push(("Decoder", format!("FFmpeg's {}", self.decoder)));
        rows.push(("Runs on", self.runs_on.clone()));
        if let Some(why) = &self.why_cpu {
            rows.push(("Why the CPU", why.clone()));
        }
        rows
    }
}

impl PictureStats {
    fn rows(&self) -> Vec<(&'static str, String)> {
        let frame = if self.exact {
            self.frame.to_string()
        } else {
            format!("{} (still decoding)", self.frame)
        };
        let mut rows = vec![
            ("Frame", frame),
            ("Size", format!("{}×{}", self.size[0], self.size[1])),
            ("Layout", self.layout.clone()),
            ("Colors", format!("{}, {} range", self.matrix, self.range)),
        ];
        if let Some(hdr) = &self.hdr {
            rows.push(("HDR", hdr.clone()));
        }
        rows.push(("Held in", self.memory.clone()));
        rows
    }
}

impl ViewerStats {
    fn rows(&self) -> Vec<(&'static str, String)> {
        let mut rows = vec![("Size", format!("{}×{}", self.size[0], self.size[1]))];
        if let Some(shown) = &self.shown {
            rows.push(("Picture", shown.clone()));
        }
        rows.push(("Renderer", self.backend.clone()));
        rows.push(("GPU", self.gpu.clone()));
        if !self.driver.is_empty() {
            rows.push(("Driver", self.driver.clone()));
        }
        rows
    }
}

impl PlaybackStats {
    fn rows(&self) -> Vec<(&'static str, String)> {
        let copied = |c: CopiedStats| format!("{} frames, {} MB", c.frames, c.bytes >> 20);
        vec![
            ("Shown", format!("{} frames", self.frames)),
            ("Late", self.late.to_string()),
            ("Skipped", self.skipped.to_string()),
            ("Copied down", copied(self.copied_down)),
            ("Copied up", copied(self.copied_up)),
        ]
    }
}

impl KeptStats {
    fn rows(&self) -> Vec<(&'static str, String)> {
        let mb = |[used, budget]: [usize; 2]| format!("{used} of {budget} MB");
        let (memory, video) = if self.shared {
            let [[memory, budget], [video, _]] = [self.memory_mb, self.video_mb];
            (
                format!("{} of {budget} MB, video memory included", memory + video),
                format!("{video} MB"),
            )
        } else {
            (mb(self.memory_mb), mb(self.video_mb))
        };
        vec![
            ("Ahead", format!("{} pictures", self.ahead)),
            ("Behind", format!("{} pictures", self.behind)),
            ("All", format!("{} pictures", self.pictures)),
            ("Memory", memory),
            ("Video memory", video),
        ]
    }
}

impl SoundStats {
    fn rows(&self) -> Vec<(&'static str, String)> {
        let mut rows = vec![
            ("Device", self.device.clone()),
            ("Rate", format!("{} Hz", self.rate)),
            ("Channels", self.channels.to_string()),
        ];
        if let Some(ms) = self.latency_ms {
            rows.push(("Latency", format!("{ms:.0} ms")));
        }
        rows
    }
}

/// A rate as people write it: `30`, `29.97`.
fn fps(rate: f64) -> String {
    let text = format!("{rate:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_has_a_row_for_each_fact() {
        let sound = SoundStats {
            device: "System default".into(),
            rate: 48000,
            channels: 2,
            latency_ms: Some(20.0),
        };
        assert_eq!(
            sound.rows(),
            [
                ("Device", "System default".to_string()),
                ("Rate", "48000 Hz".into()),
                ("Channels", "2".into()),
                ("Latency", "20 ms".into()),
            ]
        );
    }

    #[test]
    fn decoding_leaves_out_what_does_not_apply() {
        let decoding = DecodingStats {
            codec: "h264".into(),
            profile: Some("High".into()),
            fps: Some(30000.0 / 1001.0),
            interlaced: false,
            rotation: 0,
            decoder: "h264".into(),
            runs_on: "CPU".into(),
            why_cpu: None,
        };
        assert_eq!(
            decoding.rows(),
            [
                ("Codec", "h264 High".to_string()),
                ("Frame rate", "29.97 fps".into()),
                ("Decoder", "FFmpeg's h264".into()),
                ("Runs on", "CPU".into()),
            ]
        );
    }
}
