use serde::{Deserialize, Serialize};
use vidcut_av::color::Matrix;
use vidcut_av::player::{self as engine, Layout, PlaysOn};
use vidcut_av::{ai, interpolate};
use vidcut_core::model::{Hdr, Retime, ScaleFactor, Sharpness, SuperScale, Upscaler};

pub type Group = (&'static str, Vec<(&'static str, String)>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerStats {
    pub decoding: Option<DecodingStats>,
    pub picture: Option<PictureStats>,
    pub super_scale: Option<SuperScaleStats>,
    pub retime: Option<RetimeStats>,
    pub viewer: Option<ViewerStats>,
    pub playback: PlaybackStats,
    pub kept: KeptStats,
    pub sound: SoundStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodingStats {
    pub codec: String,
    pub profile: Option<String>,
    pub fps: Option<f64>,
    pub interlaced: bool,
    pub rotation: u16,
    pub decoder: String,
    pub runs_on: String,
    pub why_cpu: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PictureStats {
    pub frame: i64,
    pub exact: bool,
    pub size: [u32; 2],
    pub layout: String,
    pub matrix: String,
    pub range: String,
    pub hdr: Option<String>,
    pub memory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuperScaleStats {
    pub upscaler: Option<String>,
    pub factor: String,
    pub runs_on: Option<String>,
    pub sizes: Option<String>,
    pub state: Option<String>,
    pub last: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetimeStats {
    pub process: String,
    pub runs_on: Option<String>,
    pub last: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TwoUpStats {
    pub shown: Option<TwoUpShown>,
    pub asked: u64,
    pub passed_over: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TwoUpShown {
    pub mode: String,
    pub cells: Vec<Option<TwoUpCell>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TwoUpCell {
    pub frame: i64,
    pub source_frame: i64,
    pub exact: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewerStats {
    pub size: [u32; 2],
    pub shown: Option<String>,
    pub backend: String,
    pub gpu: String,
    pub driver: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackStats {
    pub frames: u64,
    pub late: u64,
    pub skipped: u64,
    pub copied_down: CopiedStats,
    pub copied_up: CopiedStats,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopiedStats {
    pub frames: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeptStats {
    pub ahead: usize,
    pub behind: usize,
    pub pictures: usize,
    pub memory_mb: [usize; 2],
    pub video_mb: [usize; 2],
    pub shared: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundStats {
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
        let super_scale = SuperScaleStats::of(s.super_scales.first().copied(), &ai::report());
        let retime = RetimeStats::of(s.retimes.first().copied(), &interpolate::report());
        Self {
            decoding,
            picture,
            super_scale,
            retime,
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

    pub fn groups(&self) -> Vec<Group> {
        let mut groups = Vec::new();
        if let Some(d) = &self.decoding {
            groups.push(("Decoding", d.rows()));
        }
        if let Some(p) = &self.picture {
            groups.push(("Picture", p.rows()));
        }
        if let Some(s) = &self.super_scale {
            groups.push(("Super Scale", s.rows()));
        }
        if let Some(r) = &self.retime {
            groups.push(("Retime Process", r.rows()));
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

impl RetimeStats {
    fn of(retime: Option<Retime>, report: &interpolate::Report) -> Option<Self> {
        let retime = retime?;
        let last = report
            .made
            .as_ref()
            .filter(|m| m.retime == retime)
            .map(|m| {
                let (w, h) = m.size;
                match m.took {
                    Some(took) => format!(
                        "{w}×{h} at {:.2} in {:.0} ms",
                        m.at,
                        took.as_secs_f64() * 1e3
                    ),
                    None => format!("Nearer frame at {:.2}: a shot change", m.at),
                }
            });
        Some(Self {
            process: retime.title().to_string(),
            runs_on: report.runs_on.clone(),
            last,
        })
    }

    fn rows(&self) -> Vec<(&'static str, String)> {
        let mut rows = vec![("Process", self.process.clone())];
        for (label, fact) in [("Runs on", &self.runs_on), ("Last made", &self.last)] {
            if let Some(f) = fact {
                rows.push((label, f.clone()));
            }
        }
        rows
    }
}

impl SuperScaleStats {
    fn of(s: Option<SuperScale>, report: &ai::Report) -> Option<Self> {
        if s.is_none() && report.making.is_none() {
            return None;
        }
        let factor = match s.map(|s| s.factor) {
            None | Some(ScaleFactor::None) => "Off".to_string(),
            Some(ScaleFactor::Auto) => "Auto".to_string(),
            Some(f) => f.times().map_or_else(String::new, |n| format!("{n}x")),
        };
        let upscaler = s.map(|s| match s.upscaler {
            Upscaler::Classic => format!(
                "Classic, sharpness {}",
                match s.sharpness {
                    Sharpness::Low => "low",
                    Sharpness::Medium => "medium",
                    Sharpness::High => "high",
                }
            ),
            u => ai::name(u).to_string(),
        });
        let mine = |w: &Option<ai::Work>| {
            w.clone()
                .filter(|w| s.map(|s| s.upscaler) == Some(w.upscaler))
        };
        let (making, made) = (mine(&report.making), mine(&report.made));
        let working = s.map_or(report.making.as_ref().map(|w| w.upscaler), |s| {
            Some(s.upscaler)
        });
        let size = |(w, h): (u32, u32)| format!("{w}×{h}");
        let frames = |(a, b): (i64, i64)| match a == b {
            true => format!("Source frame {a}"),
            false => format!("Source frames {a}–{b}"),
        };
        let secs = |d: std::time::Duration| format!("{:.1} s", d.as_secs_f64());
        Some(Self {
            upscaler,
            factor,
            runs_on: report
                .devices
                .iter()
                .find(|(u, _)| Some(*u) == working)
                .map(|(_, d)| d.clone()),
            sizes: making
                .as_ref()
                .or(made.as_ref())
                .or(report.making.as_ref().filter(|_| s.is_none()))
                .map(|w| format!("{} → {}", size(w.from), size(w.made))),
            state: match (&making, &report.making) {
                (Some(w), _) => Some(format!(
                    "Making {} · {} so far",
                    frames(w.frames).to_lowercase(),
                    secs(w.started.elapsed())
                )),
                (None, Some(w)) => Some(format!(
                    "{} still making {} · {} so far",
                    ai::name(w.upscaler),
                    frames(w.frames).to_lowercase(),
                    secs(w.started.elapsed())
                )),
                (None, None) => s.is_some_and(|s| s.ai()).then(|| "Idle".into()),
            },
            last: made.map(|w| {
                format!(
                    "{} in {}",
                    frames(w.frames),
                    secs(w.took.unwrap_or_default())
                )
            }),
        })
    }

    fn rows(&self) -> Vec<(&'static str, String)> {
        let mut rows = Vec::new();
        if let Some(u) = &self.upscaler {
            rows.push(("Upscaler", u.clone()));
        }
        rows.push(("Factor", self.factor.clone()));
        for (label, fact) in [
            ("Runs on", &self.runs_on),
            ("Sizes", &self.sizes),
            ("State", &self.state),
            ("Last made", &self.last),
        ] {
            if let Some(f) = fact {
                rows.push((label, f.clone()));
            }
        }
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
