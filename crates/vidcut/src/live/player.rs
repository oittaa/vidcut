//! The live session's player: a render plan decoded and mixed in-process.
//!
//! A seek or a load decodes the frame the sampling rule picks, then the
//! renderer is asked to present it. The motion is on screen once that
//! present finishes. Forward playback pulls the mix on a clock, and the
//! picture advances with those samples. Reverse is stepped seeks.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vidcut_av::thumbs::Image;
use vidcut_core::render::{LaneTrack, RenderPlan};
use vidcut_core::time::Rational;

use super::playback::Playback;
use super::snapshot::{PlayerId, PlayerSnap};

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
/// one in flight at a time.
#[derive(Debug, Clone, Copy)]
struct Reverse {
    speed: f64,
    started: Instant,
    from: i64,
}

/// The last mix buffer, and the loudest peak since the last seek.
#[derive(Debug, Clone, Copy, Default)]
pub struct Monitor {
    pub peak: f32,
    pub rms: f32,
    pub samples: usize,
    pub seq: u64,
    /// Loudest peak of the buffers pulled since the last seek.
    pub since_seek_peak: f32,
}

pub struct Player {
    pub id: PlayerId,
    play: Option<vidcut_av::play::Player>,
    plan: Option<RenderPlan>,
    /// Audio lanes, in order. Each fader is already inside the plan's gains.
    lanes: Vec<LaneTrack>,
    /// Where pull's samples go (the device, or a test recorder).
    playback: Box<dyn Playback>,
    picture: Option<Arc<Image>>,
    fps: Rational,
    rate: u32,
    muted: bool,
    pub frames: i64,
    pub position: i64,
    /// The frame the latest seek or load asked for, and whether exactly (a
    /// fast seek is marked inexact: another exact seek to the same frame is
    /// a different request). The picture is always the frame asked for.
    pub target: (i64, bool),
    speed: f64,
    reverse: Option<Reverse>,
    /// When the forward clock last pulled.
    clock: Option<Instant>,
    next_id: u64,
    /// Last seek or load.
    pub motion: u64,
    /// Motions up to this id have finished decoding.
    restarted: u64,
    /// The motion the renderer was last asked to present.
    presenting: u64,
    /// Motions up to this id are on screen.
    pub shown: u64,
    monitor: Monitor,
    pub error: Option<String>,
}

impl Player {
    pub fn new(id: PlayerId, playback: Box<dyn Playback>) -> Self {
        Self {
            id,
            play: None,
            plan: None,
            lanes: Vec::new(),
            playback,
            picture: None,
            fps: Rational::new(30, 1).expect("valid"),
            rate: 48_000,
            muted: false,
            frames: 0,
            position: 0,
            target: (0, true),
            speed: 0.0,
            reverse: None,
            clock: None,
            next_id: 1,
            motion: 0,
            restarted: 0,
            presenting: 0,
            shown: 0,
            monitor: Monitor::default(),
            error: None,
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.play.is_some()
    }

    pub fn playing(&self) -> bool {
        self.speed != 0.0
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    pub fn monitor(&self) -> Monitor {
        self.monitor
    }

    /// The current picture, or a black pixel when nothing is loaded.
    pub fn image(&self) -> Image {
        match &self.picture {
            Some(p) => (**p).clone(),
            None => Image {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            },
        }
    }

    /// Records a motion. The renderer still has to present it.
    fn moved(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.motion = id;
        self.restarted = id;
        if debug() {
            eprintln!("[{:?}] motion {id} at frame {}", self.id, self.position);
        }
        id
    }

    /// A motion with nothing to draw: already on screen.
    fn instant_motion(&mut self) -> u64 {
        let id = self.moved();
        self.presenting = id;
        self.shown = id;
        id
    }

    fn forget_buffer(&mut self) {
        self.monitor.peak = 0.0;
        self.monitor.rms = 0.0;
        self.monitor.since_seek_peak = 0.0;
    }

    /// Shows `plan` (its lanes named by `lanes`) with the playhead at
    /// `position`. `position` may be one past the last frame. Returns the
    /// motion id, or `None` when that picture is already showing.
    ///
    /// `lanes` are the tracks whose faders are already baked into the piece
    /// gains. A later [`set_lane_db`](Self::set_lane_db) replaces that fader.
    pub fn load_plan(
        &mut self,
        plan: RenderPlan,
        position: i64,
        lanes: Vec<LaneTrack>,
    ) -> Option<u64> {
        let frames = plan.frames.max(0);
        let position = position.clamp(0, frames);
        if self.plan.as_ref() == Some(&plan) && self.lanes == lanes {
            self.frames = frames;
            if position == self.position || position == self.target.0 {
                return None;
            }
            return Some(self.seek(position, false));
        }
        let keep_target = self.plan.is_some() && position == self.target.0;
        self.fps = plan.fps;
        self.rate = plan.sample_rate;
        self.playback.configure(plan.sample_rate, plan.channels);
        self.frames = frames;
        self.position = position;
        if !keep_target {
            self.target = (position, true);
        }
        self.reverse = None;
        self.lanes = lanes;
        self.forget_buffer();
        if self.speed > 0.0 {
            self.clock = Some(Instant::now());
        }
        match open_player(&plan, position, self.speed) {
            Ok((play, picture)) => {
                self.play = Some(play);
                self.picture = Some(picture);
                self.plan = Some(plan);
                self.error = None;
            }
            Err(e) => {
                self.play = None;
                self.picture = None;
                self.plan = None;
                self.error = Some(e);
            }
        }
        Some(self.moved())
    }

    /// Shows nothing.
    pub fn unload(&mut self) {
        self.play = None;
        self.plan = None;
        self.picture = None;
        self.lanes.clear();
        self.frames = 0;
        self.position = 0;
        self.target = (0, true);
        self.speed = 0.0;
        self.reverse = None;
        self.clock = None;
        self.instant_motion();
    }

    /// Moves the playhead to `frame` (clamped to the content, including the
    /// frame just past the end). `fast` marks the request inexact. Returns
    /// the motion id.
    pub fn seek(&mut self, frame: i64, fast: bool) -> u64 {
        if self.play.is_none() {
            self.position = frame.max(0);
            self.target = (self.position, !fast);
            self.forget_buffer();
            return self.instant_motion();
        }
        let frame = frame.clamp(0, self.frames.max(0));
        self.position = frame;
        self.target = (frame, !fast);
        self.forget_buffer();
        let show = frame.min(self.frames.saturating_sub(1).max(0));
        if let Some(play) = self.play.as_mut() {
            match play.seek(show) {
                Ok(()) => {
                    self.picture = Some(Arc::new(play.picture().clone()));
                    self.error = None;
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        self.moved()
    }

    /// Plays at `speed` (negative: stepped reverse). At the end already,
    /// forward play stays paused: there is no frame left to show.
    pub fn play(&mut self, speed: f64) {
        if self.play.is_none() || speed == 0.0 {
            return;
        }
        if speed > 0.0 && self.frames > 0 && self.position >= self.frames {
            return;
        }
        self.speed = speed;
        if speed > 0.0 {
            self.reverse = None;
            self.clock = Some(Instant::now());
            if let Some(play) = self.play.as_mut()
                && let Err(e) = play.set_speed(speed)
            {
                self.error = Some(e.to_string());
            }
        } else if let Some(play) = self.play.as_mut() {
            if let Err(e) = play.set_speed(0.0) {
                self.error = Some(e.to_string());
            }
            self.clock = None;
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
        self.clock = None;
        if let Some(play) = self.play.as_mut()
            && let Err(e) = play.set_speed(0.0)
        {
            self.error = Some(e.to_string());
        }
    }

    /// Mutes what this player would play (the monitoring mute).
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// Sets the fader of `track` to `db` (its position, not an extra on top
    /// of the saved one). The plan's piece gains already include the saved
    /// fader, so the lane trim is the difference. Heard on the next buffer.
    pub fn set_lane_db(&mut self, track: &str, db: f64) -> Result<(), String> {
        let Some((i, lane)) = self
            .lanes
            .iter()
            .enumerate()
            .find(|(_, lane)| lane.track == track)
        else {
            return Err(format!("no audio for track {track}"));
        };
        let extra = db - lane.volume_db;
        let Some(play) = self.play.as_mut() else {
            return Err("nothing is loaded".into());
        };
        play.set_lane_db(i, extra).map_err(|e| e.to_string())
    }

    /// The motion to ask the renderer to present, once.
    pub fn present_due(&mut self) -> Option<u64> {
        let due = self.restarted >= self.motion
            && self.shown < self.motion
            && self.presenting < self.motion;
        due.then(|| {
            self.presenting = self.motion;
            self.motion
        })
    }

    /// The renderer presented this player after motion `motion` finished.
    pub fn presented(&mut self, motion: u64) {
        if self.restarted >= motion {
            self.shown = self.shown.max(motion);
        }
    }

    /// Timed work: the forward mix, and stepped reverse. Returns when it
    /// next needs to run.
    pub fn tick(&mut self, now: Instant) -> Option<Instant> {
        if let Some(r) = self.reverse {
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
            return Some(now + step.min(Duration::from_millis(40)));
        }
        if self.speed > 0.0 && self.play.is_some() {
            let last = self.clock.replace(now).unwrap_or(now);
            let dt = now.saturating_duration_since(last).as_secs_f64();
            let n = (dt * f64::from(self.rate.max(1))).round() as usize;
            let n = n.min(self.rate.max(1) as usize);
            if n > 0 {
                self.pull_forward(n);
            }
            if self.speed > 0.0 {
                return Some(now + Duration::from_millis(20));
            }
        }
        None
    }

    fn pull_forward(&mut self, n: usize) {
        let outcome = {
            let Some(play) = self.play.as_mut() else {
                return;
            };
            match play.pull(n) {
                Ok(pulled) => {
                    let frame = play.frame();
                    let picture = pulled
                        .frame
                        .is_some()
                        .then(|| Arc::new(play.picture().clone()));
                    Ok((frame, picture, pulled.samples, pulled.ended))
                }
                Err(e) => Err(e.to_string()),
            }
        };
        match outcome {
            Ok((frame, picture, samples, ended)) => {
                self.position = frame;
                if let Some(p) = picture {
                    self.picture = Some(p);
                }
                self.note_audio(&samples);
                if self.muted {
                    self.playback.write(&vec![0.0; samples.len()]);
                } else {
                    self.playback.write(&samples);
                }
                if ended {
                    self.stop_at_end(frame);
                }
            }
            Err(e) => {
                self.error = Some(e);
                self.speed = 0.0;
                self.clock = None;
                if let Some(play) = self.play.as_mut() {
                    let _ = play.set_speed(0.0);
                }
            }
        }
    }

    /// Forward play reached the end and paused itself. The playhead rests on
    /// the frame just past the last one; the picture stays on the last frame.
    fn stop_at_end(&mut self, frame: i64) {
        self.speed = 0.0;
        self.reverse = None;
        self.clock = None;
        self.position = frame.saturating_add(1).clamp(0, self.frames.max(0));
        if let Some(play) = self.play.as_mut() {
            let _ = play.set_speed(0.0);
        }
    }

    fn note_audio(&mut self, samples: &[f32]) {
        let (mut peak, mut sum) = (0.0f32, 0.0f64);
        for &s in samples {
            peak = peak.max(s.abs());
            sum += f64::from(s) * f64::from(s);
        }
        let rms = if samples.is_empty() {
            0.0
        } else {
            (sum / samples.len() as f64).sqrt() as f32
        };
        if self.muted {
            peak = 0.0;
        }
        let rms = if self.muted { 0.0 } else { rms };
        self.monitor.peak = peak;
        self.monitor.rms = rms;
        self.monitor.samples = samples.len();
        self.monitor.seq += 1;
        self.monitor.since_seek_peak = self.monitor.since_seek_peak.max(peak);
    }

    pub fn snap(&self) -> PlayerSnap {
        let video_size = self.picture.as_ref().map(|p| (p.width, p.height));
        PlayerSnap {
            loaded: self.is_loaded(),
            muted: self.muted,
            frames: self.frames,
            position: self.position,
            playing: self.speed != 0.0,
            speed: self.speed,
            hwdec: String::new(),
            video_size,
            error: self.error.clone(),
            picture: self.picture.clone(),
        }
    }
}

/// Opens `plan` at `position` (the picture is the last frame when `position`
/// is the end). A positive `speed` keeps playing.
fn open_player(
    plan: &RenderPlan,
    position: i64,
    speed: f64,
) -> Result<(vidcut_av::play::Player, Arc<Image>), String> {
    let mut play = vidcut_av::play::Player::new(plan.clone()).map_err(|e| e.to_string())?;
    let show = position.min(plan.frames.saturating_sub(1).max(0));
    play.seek(show).map_err(|e| e.to_string())?;
    if speed > 0.0 {
        play.set_speed(speed).map_err(|e| e.to_string())?;
    }
    let picture = Arc::new(play.picture().clone());
    Ok((play, picture))
}

/// `VIDCUT_DEBUG_PLAYER=1` traces player commands to stderr.
pub fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VIDCUT_DEBUG_PLAYER").is_some())
}

#[cfg(test)]
mod tests {
    use vidcut_core::render::{RenderPlan, VideoPiece};
    use vidcut_core::time::Rational;

    use std::sync::{Arc, Mutex};

    use crate::live::playback::{Discard, Recorder};
    use crate::live::snapshot::PlayerId;

    use super::*;

    fn fps() -> Rational {
        Rational::new(30, 1).unwrap()
    }

    fn plan(name: &str) -> RenderPlan {
        RenderPlan {
            fps: fps(),
            width: 16,
            height: 16,
            sample_rate: 48_000,
            channels: 2,
            frames: 300,
            video: vec![VideoPiece::Black { frames: 300 }],
            audio: Vec::new(),
            range: (0, 300),
            name: name.into(),
        }
    }

    fn player() -> Player {
        Player::new(PlayerId::Timeline, Box::new(Discard))
    }

    fn recording() -> (Player, Arc<Mutex<Vec<f32>>>) {
        let samples = Arc::new(Mutex::new(Vec::new()));
        let player = Player::new(
            PlayerId::Timeline,
            Box::new(Recorder {
                samples: samples.clone(),
            }),
        );
        (player, samples)
    }

    #[test]
    fn a_fast_seek_is_marked_inexact_and_an_exact_one_is_not() {
        let mut p = player();
        p.load_plan(plan("a"), 0, Vec::new()).unwrap();
        p.seek(100, true);
        assert_eq!(p.target, (100, false));
        assert_eq!(p.position, 100);
        p.seek(100, false);
        assert_eq!(p.target, (100, true));
    }

    #[test]
    fn a_reload_at_the_requested_frame_keeps_the_target() {
        let mut p = player();
        p.load_plan(plan("a"), 0, Vec::new()).unwrap();
        p.seek(40, true);
        // Playback has moved the playhead off the seek.
        p.position = 80;
        assert_eq!(p.target, (40, false));
        // Same picture: not a seek back to the request.
        assert!(p.load_plan(plan("a"), 40, Vec::new()).is_none());
        assert_eq!(p.target, (40, false));
        assert_eq!(p.position, 80);
        // The plan changed, still opening at the frame the seek asked for.
        p.load_plan(plan("b"), 40, Vec::new()).unwrap();
        assert_eq!(p.target, (40, false));
        assert_eq!(p.position, 40);
    }

    #[test]
    fn a_reload_at_another_frame_is_a_new_exact_target() {
        let mut p = player();
        p.load_plan(plan("a"), 0, Vec::new()).unwrap();
        p.seek(40, true);
        p.position = 80;
        p.load_plan(plan("b"), 80, Vec::new()).unwrap();
        assert_eq!(p.target, (80, true));
        assert_eq!(p.position, 80);
    }

    #[test]
    fn the_first_open_is_exact_even_at_a_remembered_fast_seek() {
        let mut p = player();
        p.seek(10, true);
        assert_eq!(p.target, (10, false));
        p.load_plan(plan("a"), 10, Vec::new()).unwrap();
        assert_eq!(p.target, (10, true));
    }

    #[test]
    fn forward_play_stops_after_the_last_frame() {
        let mut p = player();
        let mut short = plan("end");
        short.frames = 30;
        short.range = (0, 30);
        short.video = vec![VideoPiece::Black { frames: 30 }];
        p.load_plan(short, 0, Vec::new()).unwrap();
        p.play(1.0);
        let t0 = Instant::now();
        // A few milliseconds of a one-second plan does not finish it.
        assert!(p.tick(t0 + Duration::from_millis(20)).is_some());
        assert!(p.playing(), "stopped before the end");
        assert!(p.position < 30, "playhead jumped to {}", p.position);
        let mut now = t0 + Duration::from_millis(20);
        let mut stopped = false;
        for _ in 0..5 {
            now += Duration::from_secs(1);
            let next = p.tick(now);
            if !p.playing() {
                assert!(next.is_none(), "a finished play kept the clock");
                stopped = true;
                break;
            }
        }
        assert!(stopped, "forward play never paused at the end");
        assert_eq!(p.position, 30, "the playhead did not rest at the end");
        // Playing again at the end stays paused.
        p.play(1.0);
        assert!(!p.playing());
    }

    #[test]
    fn mute_writes_silence_to_the_sink() {
        let (mut p, heard) = recording();
        p.load_plan(plan("a"), 0, Vec::new()).unwrap();
        p.play(1.0);
        let t0 = Instant::now();
        p.tick(t0 + Duration::from_millis(40));
        let open = heard.lock().unwrap().len();
        assert!(open > 0, "playback wrote nothing");
        p.set_muted(true);
        p.tick(t0 + Duration::from_millis(80));
        let buf = heard.lock().unwrap();
        assert!(buf.len() > open, "mute wrote nothing");
        assert!(
            buf[open..].iter().all(|s| *s == 0.0),
            "mute left sound in the sink"
        );
    }
}
