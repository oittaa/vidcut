//! One mpv player as the live session drives it: what is loaded, where the
//! playhead is, and when a requested frame is actually on screen.
//!
//! Only the session thread calls into mpv's client API (docs/decisions/preview.md
//! §6); the UI thread renders. Every command is asynchronous and gets an id.
//! mpv runs them in order, so a reply to command `n` means every command up to
//! `n` has been queued. A seek or load ("motion") with id `m` has finished
//! when a `playback-restart` arrives after the seek (or file start) that mpv
//! began after replying to `m`. mpv's current picture is then the motion's
//! first frame, and it is on screen once the renderer has presented the
//! player: drawn its current picture after that restart (docs/decisions/gui.md
//! §4).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use vidcut_core::preview::Preview;
use vidcut_core::time::Rational;
use vidcut_mpv::{Event, Format, Mpv, Value};

use super::snapshot::{PlayerId, PlayerSnap};

/// Properties every player observes.
const OBSERVED: &[(&str, Format)] = &[
    ("time-pos", Format::Double),
    ("pause", Format::Flag),
    ("hwdec-current", Format::String),
    ("video-params/dw", Format::Int),
    ("video-params/dh", Format::Int),
];

/// mpv options for preview players (docs/decisions/preview.md §5).
/// `VIDCUT_MPV_OPTIONS="hwdec=no,ao=null"` adds or overrides options (for
/// debugging).
pub fn mpv_options(audio: bool) -> Vec<(String, String)> {
    let mut o: Vec<(String, String)> = vec![("hwdec".into(), "auto-safe".into())];
    for (k, v) in [
        ("vo", "libmpv"),
        ("config", "no"),
        ("load-scripts", "no"),
        ("ytdl", "no"),
        ("terminal", "no"),
        ("input-default-bindings", "no"),
        ("osc", "no"),
        ("hr-seek", "yes"),
        ("hr-seek-framedrop", "no"),
        ("keep-open", "always"),
        ("idle", "yes"),
        ("pause", "yes"),
        ("video-timing-offset", "0"),
        ("audio-client-name", "vidcut"),
    ] {
        o.push((k.into(), v.into()));
    }
    if !audio {
        o.push(("ao".into(), "null".into()));
    }
    if let Ok(extra) = std::env::var("VIDCUT_MPV_OPTIONS") {
        for kv in extra.split(',').filter(|s| !s.is_empty()) {
            if let Some((k, v)) = kv.split_once('=') {
                o.push((k.trim().into(), v.trim().into()));
            }
        }
    }
    o
}

/// Whether `path` is on a network file system (NFS, SMB/CIFS, Ceph, AFS,
/// 9P, or FUSE, as GNOME's mounted shares and sshfs are). Can block while a
/// share doesn't answer: call it off the session thread.
pub fn on_network(path: &std::path::Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    unsafe { libc::statfs(c.as_ptr(), &mut fs) == 0 && network_fs(fs.f_type as i64) }
}

#[cfg(test)]
mod storage_tests {
    use super::network_fs;

    #[test]
    fn network_file_systems_are_known_by_their_magic() {
        assert!(network_fs(0x6969), "NFS");
        assert!(network_fs(0xfe53_4d42), "SMB2");
        assert!(
            network_fs(0xfe53_4d42_u32 as i32 as i64),
            "SMB2, sign-extended"
        );
        assert!(network_fs(0x6573_5546), "FUSE");
        assert!(!network_fs(0xef53), "ext4");
        assert!(!network_fs(0x9123_683e), "btrfs");
        assert!(!network_fs(0x0102_1994), "tmpfs");
    }
}

/// Whether a `statfs` file system type is a network one.
fn network_fs(f_type: i64) -> bool {
    const NETWORK: [i64; 9] = [
        0x6969,      // NFS
        0x517b,      // SMB
        0xff53_4d42, // CIFS
        0xfe53_4d42, // SMB2
        0x564c,      // NCP
        0x00c3_6400, // Ceph
        0x5346_414f, // AFS
        0x0102_1997, // 9P
        0x6573_5546, // FUSE
    ];
    // Some C libraries sign-extend the 32-bit magic.
    NETWORK.contains(&f_type) || NETWORK.contains(&(f_type & 0xffff_ffff))
}

/// Stepped reverse playback (J): exact seeks backwards at the shuttle speed,
/// one in flight at a time (docs/decisions/preview.md §7).
#[derive(Debug, Clone, Copy)]
struct Reverse {
    speed: f64,
    started: Instant,
    from: i64,
}

pub struct Player {
    pub id: PlayerId,
    mpv: Option<Mpv>,
    edl_path: PathBuf,
    fps: Rational,
    /// The EDL and audio graph loaded (or being loaded).
    loaded: Option<Preview>,
    /// The monitoring mute (p. 642).
    muted: bool,
    /// The load command while mpv opens the file (it rejects seeks until then).
    loading: Option<u64>,
    pub frames: i64,
    pub position: i64,
    /// The playhead follows mpv's `time-pos` (playing) rather than the last
    /// requested frame (paused after a seek).
    tracking: bool,
    paused: bool,
    speed: f64,
    reverse: Option<Reverse>,
    next_id: u64,
    /// Last seek or load command.
    pub motion: u64,
    acked: u64,
    /// Commands acked when mpv last began a seek or a file: the next restart
    /// finishes them. (A restart of an older seek can arrive after a newer
    /// command's reply; it doesn't finish that command.)
    seeking: u64,
    /// Motions up to this id have finished.
    restarted: u64,
    /// The motion the renderer was last asked to present.
    presenting: u64,
    /// Motions up to this id are on screen.
    pub shown: u64,
    hwdec: String,
    video_size: (Option<i64>, Option<i64>),
    pub error: Option<String>,
}

impl Player {
    pub fn new(id: PlayerId, mpv: Option<Mpv>, edl_path: PathBuf) -> Self {
        if let Some(m) = &mpv {
            for (i, (name, format)) in OBSERVED.iter().enumerate() {
                let _ = m.observe_property(i as u64 + 1, name, *format);
            }
        }
        Self {
            id,
            mpv,
            edl_path,
            fps: Rational::new(30, 1).expect("valid"),
            loaded: None,
            muted: false,
            loading: None,
            frames: 0,
            position: 0,
            tracking: false,
            paused: true,
            speed: 0.0,
            reverse: None,
            next_id: 1,
            motion: 0,
            acked: 0,
            seeking: 0,
            restarted: 0,
            presenting: 0,
            shown: 0,
            hwdec: String::new(),
            video_size: (None, None),
            error: None,
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded.is_some()
    }

    pub fn playing(&self) -> bool {
        self.speed != 0.0
    }

    fn command(&mut self, args: &[&str]) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        if debug() {
            eprintln!("[{:?}] #{id} {args:?}", self.id);
        }
        if let Some(m) = &self.mpv
            && let Err(e) = m.command_async(id, args)
        {
            self.error = Some(e.to_string());
        }
        id
    }

    /// Records a motion command; without mpv it completes at once.
    fn moved(&mut self, id: u64) -> u64 {
        self.motion = id;
        if self.mpv.is_none() {
            self.acked = id;
            self.restarted = id;
            self.shown = id;
        }
        id
    }

    fn secs(&self, frame: i64) -> String {
        format!("{:.9}", self.fps.frames_to_secs(frame.max(0)))
    }

    /// Mutes what the player plays (the monitoring mute, p. 642).
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        if let Some(m) = &self.mpv
            && let Err(e) = m.set_property_flag("mute", muted)
        {
            self.error = Some(e.to_string());
        }
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    /// Shows `preview` (content `frames` long at `fps`) with the playhead at
    /// `position`. Returns the motion id, or `None` if it is already showing.
    pub fn load(
        &mut self,
        preview: Preview,
        frames: i64,
        fps: Rational,
        position: i64,
    ) -> Option<u64> {
        let position = position.clamp(0, frames);
        if self.loaded.as_ref() == Some(&preview) && self.fps == fps {
            self.frames = frames;
            return (position != self.position).then(|| self.seek(position, false));
        }
        self.fps = fps;
        self.frames = frames;
        self.position = position;
        self.reverse = None;
        if self.mpv.is_some() {
            let tmp = self.edl_path.with_extension("edl.tmp");
            let written = std::fs::write(&tmp, &preview.edl)
                .and_then(|()| std::fs::rename(&tmp, &self.edl_path));
            if let Err(e) = written {
                self.error = Some(format!("writing {}: {e}", self.edl_path.display()));
                return None;
            }
        }
        self.loaded = Some(preview);
        self.error = None;
        Some(self.loadfile())
    }

    /// (Re)opens the EDL file at the playhead.
    fn loadfile(&mut self) -> u64 {
        let path = self.edl_path.to_string_lossy().into_owned();
        let mut options = format!(
            "start={},pause={}",
            self.secs(self.position),
            if self.speed > 0.0 { "no" } else { "yes" }
        );
        // Mixes the audio streams and applies gains (preview.md §10), for
        // this file only.
        if let Some(graph) = self.loaded.as_ref().and_then(|p| p.audio_graph.as_ref()) {
            options.push_str(&format!(",lavfi-complex=%{}%{graph}", graph.len()));
        }
        // Network shares: read ahead, and keep what was read for seeking back
        // and forth (mpv's cache stays off for local files).
        if self.loaded.as_ref().is_some_and(|p| p.read_ahead) {
            options.push_str(",cache=yes,demuxer-max-back-bytes=150MiB");
        }
        let id = self.command(&["loadfile", &path, "replace", "-1", &options]);
        self.tracking = self.speed > 0.0;
        if self.mpv.is_some() {
            self.loading = Some(id);
        }
        self.moved(id)
    }

    /// A motion that needs no command (nothing loaded): already complete.
    fn instant_motion(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.motion = id;
        self.acked = self.acked.max(id);
        self.restarted = self.restarted.max(id);
        self.shown = self.shown.max(id);
        id
    }

    /// Shows nothing.
    pub fn unload(&mut self) {
        if self.loaded.take().is_some() {
            self.command(&["stop"]);
        }
        self.loading = None;
        self.frames = 0;
        self.position = 0;
        self.speed = 0.0;
        self.reverse = None;
        self.instant_motion();
    }

    /// Moves the playhead to `frame` (clamped to the content). `fast` seeks
    /// to a keyframe (scrubbing). Returns the motion id.
    pub fn seek(&mut self, frame: i64, fast: bool) -> u64 {
        if self.loaded.is_none() {
            // Remembered for the next load (which clamps it).
            self.position = frame.max(0);
            return self.instant_motion();
        }
        let frame = frame.clamp(0, self.frames.max(0));
        self.position = frame;
        if self.loading.is_some() {
            // mpv rejects seeks while it opens the file: open it again at the
            // new position instead.
            return self.loadfile();
        }
        if self.reverse.is_none() && self.speed != 0.0 {
            // A seek while playing keeps playing from there.
            self.tracking = true;
        } else {
            self.tracking = false;
        }
        let secs = self.secs(frame);
        let mode = if fast {
            "absolute+keyframes"
        } else {
            "absolute+exact"
        };
        let id = self.command(&["seek", &secs, mode]);
        self.moved(id)
    }

    /// Plays at `speed` (negative: stepped reverse).
    pub fn play(&mut self, speed: f64) {
        if self.loaded.is_none() || speed == 0.0 {
            return;
        }
        self.speed = speed;
        if speed > 0.0 {
            self.reverse = None;
            self.tracking = true;
            let s = format!("{speed}");
            self.command(&["set", "speed", &s]);
            self.command(&["set", "pause", "no"]);
        } else {
            self.command(&["set", "pause", "yes"]);
            self.tracking = false;
            self.reverse = Some(Reverse {
                speed: -speed,
                started: Instant::now(),
                from: self.position,
            });
        }
    }

    /// Stops playback where it is.
    pub fn pause(&mut self) {
        if self.speed == 0.0 {
            return;
        }
        self.speed = 0.0;
        self.reverse = None;
        self.command(&["set", "pause", "yes"]);
        self.command(&["set", "speed", "1"]);
        // `time-pos` keeps the playhead on the frame shown when mpv paused.
    }

    /// The motion to ask the renderer to present, once: the latest one, when
    /// it has finished but isn't on screen yet.
    pub fn present_due(&mut self) -> Option<u64> {
        let due = self.restarted >= self.motion
            && self.shown < self.motion
            && self.presenting < self.motion;
        due.then(|| {
            self.presenting = self.motion;
            self.motion
        })
    }

    /// The renderer presented this player after motion `motion` finished:
    /// the viewer shows its frame.
    pub fn presented(&mut self, motion: u64) {
        if self.restarted >= motion {
            self.shown = self.shown.max(motion);
        }
    }

    /// Drains mpv's events. Returns whether anything visible changed.
    pub fn pump(&mut self) -> bool {
        let Some(mpv) = self.mpv.clone() else {
            return false;
        };
        let mut changed = false;
        while let Some(event) = mpv.wait_event(0.0) {
            changed = true;
            if debug() && !matches!(event, Event::PropertyChange { .. }) {
                eprintln!(
                    "[{:?}] {event:?} (motion {} acked {} loading {:?})",
                    self.id, self.motion, self.acked, self.loading
                );
            }
            match event {
                Event::CommandReply { id, error } => {
                    self.acked = self.acked.max(id);
                    if error < 0 && id == self.motion {
                        self.error = Some(format!("mpv command failed ({error})"));
                        // Nothing will restart for it; don't keep waiters waiting.
                        self.restarted = self.restarted.max(id);
                        self.shown = self.shown.max(id);
                    }
                }
                // mpv begins seeks (ours or a load's initial one) and files
                // only after replying to the commands that asked for them.
                Event::Seek | Event::StartFile => self.seeking = self.acked,
                Event::PlaybackRestart => {
                    self.restarted = self.restarted.max(self.seeking);
                    if self.loading.is_some_and(|l| self.restarted >= l) {
                        self.loading = None;
                    }
                }
                Event::EndFile { reason: 4, error } => {
                    self.loading = None;
                    self.error = Some(format!("playback failed ({error})"));
                    self.restarted = self.restarted.max(self.motion);
                    self.shown = self.shown.max(self.motion);
                }
                Event::PropertyChange { name, value, .. } => self.property(&name, value),
                _ => {}
            }
        }
        changed
    }

    fn property(&mut self, name: &str, value: Value) {
        match (name, value) {
            ("time-pos", Value::Double(t)) => {
                if self.tracking {
                    let f = (t * self.fps.as_f64() + 1e-6).floor() as i64;
                    self.position = f.clamp(0, self.frames.max(0));
                }
            }
            ("pause", Value::Flag(p)) => {
                self.paused = p;
                // mpv pauses by itself at the end (keep-open).
                if p && self.speed > 0.0 {
                    self.speed = 0.0;
                }
            }
            ("hwdec-current", Value::String(s)) => self.hwdec = s,
            ("video-params/dw", v) => self.video_size.0 = int(&v),
            ("video-params/dh", v) => self.video_size.1 = int(&v),
            _ => {}
        }
    }

    /// Timed work: stepped reverse. Returns when it next needs to run.
    pub fn tick(&mut self, now: Instant) -> Option<Instant> {
        let r = self.reverse?;
        let elapsed = now.duration_since(r.started).as_secs_f64();
        let target = (r.from - (elapsed * r.speed * self.fps.as_f64()) as i64).max(0);
        if self.shown >= self.motion && target < self.position {
            self.seek(target, false);
        }
        if target == 0 && self.position == 0 && self.shown >= self.motion {
            self.reverse = None;
            self.speed = 0.0;
            return None;
        }
        let step = Duration::from_secs_f64(1.0 / (self.fps.as_f64() * r.speed).max(1.0));
        Some(now + step.min(Duration::from_millis(40)))
    }

    pub fn snap(&self) -> PlayerSnap {
        PlayerSnap {
            loaded: self.loaded.is_some(),
            muted: self.muted,
            frames: self.frames,
            position: self.position,
            playing: self.speed != 0.0,
            speed: self.speed,
            hwdec: self.hwdec.clone(),
            video_size: match self.video_size {
                (Some(w), Some(h)) if w > 0 && h > 0 => Some((w as u32, h as u32)),
                _ => None,
            },
            error: self.error.clone(),
        }
    }
}

/// `VIDCUT_DEBUG_PLAYER=1` traces player commands, events and renders to
/// stderr.
pub fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VIDCUT_DEBUG_PLAYER").is_some())
}

fn int(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(*i),
        _ => None,
    }
}
