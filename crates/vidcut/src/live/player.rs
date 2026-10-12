//! One viewer's player as the live session drives it: vidcut's own engine
//! (`vidcut_av::player`, docs/decisions/player.md) playing a render plan,
//! what is loaded, where the playhead is, and when a requested frame is on
//! screen (docs/decisions/gui.md §4).
//!
//! Every seek, step and load is a motion with a number. A motion is on
//! screen once the renderer presented the player after it and showed the
//! frame it asked for exactly: paused, that frame; playing, any frame (the
//! clock starts at the frame asked for and moves on from there). The
//! session asks the renderer to present when the engine has the frame's
//! picture.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vidcut_av::player::{self as engine, AudioOutput, Decoder, PacedOutput};
use vidcut_core::render::{AudioPiece, LaneTrack, RenderPlan, VideoPiece};

use super::Renderer;
use super::snapshot::{PlayerId, PlayerSnap};

/// Makes a player's audio output: the system's device in the GUI, a
/// stand-in in tests.
pub type MakeOutput = Arc<dyn Fn(PlayerId) -> Box<dyn AudioOutput> + Send + Sync>;

/// How long a present asked for may go unanswered before it is asked again.
const PRESENT_AGAIN: Duration = Duration::from_millis(50);

pub struct Player {
    pub id: PlayerId,
    /// Makes the engine's audio output; `None`: no preview at all.
    output: Option<MakeOutput>,
    renderer: Arc<dyn Renderer>,
    /// Called on the engine's decode thread when the picture the viewer is
    /// at lands (or fails).
    on_picture: Arc<dyn Fn() + Send + Sync>,
    /// Memory for the engine's pictures.
    budget: usize,
    engine: Option<Arc<engine::Player>>,
    /// The plan the engine plays: proxies swapped in, and a frame of black
    /// after the content, where the playhead stands at its end.
    plan: Option<RenderPlan>,
    /// The tracks of the plan's audio lanes (the timeline's), for faders
    /// moved while it plays.
    lanes: Vec<LaneTrack>,
    /// Length of the content in frames (the playhead goes up to it).
    pub frames: i64,
    /// The playhead while nothing is loaded (remembered for the next load).
    position: i64,
    /// The frame the latest seek, step or load asked for. Playback leaves
    /// it alone, so a waiting seek is overtaken only by another motion.
    pub target: i64,
    /// The latest motion, and the newest one on screen.
    pub motion: u64,
    pub shown: u64,
    /// A present asked of the renderer: for which motion, and when.
    presenting: Option<(u64, Instant)>,
    muted: bool,
    scrubbing: bool,
    /// Decode on the GPU (the GPU acceleration preference).
    gpu: bool,
    /// The audio device failed to open: the player plays to a stand-in.
    pub audio_error: Option<String>,
    pub error: Option<String>,
}

impl Player {
    pub fn new(
        id: PlayerId,
        output: Option<MakeOutput>,
        budget: usize,
        renderer: Arc<dyn Renderer>,
        on_picture: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            id,
            output,
            renderer,
            on_picture,
            budget,
            engine: None,
            plan: None,
            lanes: Vec::new(),
            frames: 0,
            position: 0,
            target: 0,
            motion: 0,
            shown: 0,
            presenting: None,
            muted: false,
            scrubbing: true,
            gpu: crate::preferences::gpu_acceleration(),
            audio_error: None,
            error: None,
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.plan.is_some()
    }

    fn status(&self) -> Option<engine::Status> {
        self.engine.as_ref().map(|e| e.status())
    }

    /// The playhead: where the engine is (the clock while playing).
    pub fn position(&self) -> i64 {
        match self.status() {
            Some(s) => s.position.clamp(0, self.frames.max(0)),
            None => self.position,
        }
    }

    pub fn playing(&self) -> bool {
        self.status().is_some_and(|s| s.playing)
    }

    /// A motion that completes at once (nothing to show).
    fn instant(&mut self) -> u64 {
        self.motion += 1;
        self.shown = self.motion;
        self.motion
    }

    /// Shows `plan` (its content `plan.frames` long), with `lanes` its audio
    /// lanes' tracks. `at`: the frame to show (clamped to the content);
    /// `None` stays where the playhead is, playing on if it plays. Returns
    /// the motion, or `None` when the picture can't have changed.
    pub fn load(
        &mut self,
        mut plan: RenderPlan,
        lanes: Vec<LaneTrack>,
        at: Option<i64>,
    ) -> Option<u64> {
        let frames = plan.frames;
        with_end(&mut plan);
        if self.plan.as_ref() == Some(&plan) {
            self.frames = frames;
            let to = at?.clamp(0, frames);
            if to == self.position() && to == self.target {
                return None;
            }
            return Some(self.seek(to, false));
        }
        self.frames = frames;
        self.lanes = lanes;
        self.error = None;
        let at = at.map(|f| f.clamp(0, frames));
        let format = (plan.sample_rate, plan.channels);
        let reuse = self
            .plan
            .as_ref()
            .is_some_and(|p| (p.sample_rate, p.channels) == format);
        match &self.engine {
            Some(e) if reuse => {
                if let Err(e) = e.set_plan(plan.clone()) {
                    self.error = Some(e.to_string());
                    return None;
                }
                if let Some(f) = at {
                    e.seek_quietly(f);
                    self.target = f;
                } else {
                    self.target = self.target.clamp(0, frames);
                }
            }
            _ => {
                let from = at.unwrap_or(self.position).clamp(0, frames);
                let engine = self.open(plan.clone())?;
                engine.seek_quietly(from);
                engine.set_audio_scrubbing(self.scrubbing);
                engine.set_muted(self.muted);
                self.engine = Some(engine);
                self.renderer.attach(self.id, self.engine.clone());
                self.target = from;
            }
        }
        self.plan = Some(plan);
        self.motion += 1;
        Some(self.motion)
    }

    /// A new engine for `plan`, paused at its first frame. Its sound goes to
    /// the output made for it, or to a silent stand-in when that won't open
    /// (no audio device): the picture plays all the same.
    fn open(&mut self, plan: RenderPlan) -> Option<Arc<engine::Player>> {
        let make = self.output.clone()?;
        let config = || engine::Config {
            budget: self.budget,
            // Moves the session makes by itself play no sound; on after.
            audio_scrubbing: false,
            on_picture: Some(self.on_picture.clone()),
            hardware: self.gpu,
        };
        self.audio_error = None;
        match engine::Player::new(plan.clone(), config(), make(self.id)) {
            Ok(e) => return Some(Arc::new(e)),
            Err(e) => self.audio_error = Some(e.to_string()),
        }
        let silent = Box::new(PacedOutput::new(Duration::from_millis(10), Duration::ZERO));
        match engine::Player::new(plan, config(), silent) {
            Ok(e) => Some(Arc::new(e)),
            Err(e) => {
                self.error = Some(e.to_string());
                None
            }
        }
    }

    /// Shows nothing.
    pub fn unload(&mut self) {
        if self.engine.take().is_some() {
            self.renderer.attach(self.id, None);
        }
        self.plan = None;
        self.lanes.clear();
        self.frames = 0;
        self.position = 0;
        self.target = 0;
        self.error = None;
        self.instant();
    }

    /// Moves the playhead to `frame` (clamped to the content). Paused, the
    /// frame's sound plays when `sound` and audio scrubbing are on (a drag
    /// of the playhead is seeks); playing, play goes on from there.
    /// Returns the motion.
    pub fn seek(&mut self, frame: i64, sound: bool) -> u64 {
        let Some(engine) = self.engine.clone() else {
            // Remembered for the next load (which clamps it).
            self.position = frame.max(0);
            self.target = self.position;
            return self.instant();
        };
        let frame = frame.clamp(0, self.frames.max(0));
        self.target = frame;
        if sound {
            engine.seek(frame);
        } else {
            engine.seek_quietly(frame);
        }
        self.motion += 1;
        self.motion
    }

    /// Pauses and moves `frames` from where the viewer is, playing the
    /// frame's sound when scrubbing. Returns the motion.
    pub fn step(&mut self, frames: i64) -> u64 {
        let Some(engine) = self.engine.clone() else {
            let to = self.position.saturating_add(frames).max(0);
            return self.seek(to, false);
        };
        engine.step(frames);
        self.target = engine.status().position;
        self.motion += 1;
        self.motion
    }

    /// Plays at `speed` (negative: backward, the sound reversed).
    pub fn play(&mut self, speed: f64) {
        if let Some(e) = &self.engine {
            e.play(speed);
        }
    }

    /// Stops where the viewer is.
    pub fn pause(&mut self) {
        if let Some(e) = &self.engine {
            e.pause();
        }
    }

    /// The monitoring mute.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        if let Some(e) = &self.engine {
            e.set_muted(muted);
        }
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    /// Audio scrubbing (decision D42): steps and paused seeks play their
    /// frame's sound.
    pub fn set_scrubbing(&mut self, on: bool) {
        self.scrubbing = on;
        if let Some(e) = &self.engine {
            e.set_audio_scrubbing(on);
        }
    }

    /// Decodes as the GPU acceleration preference `gpu` says; whether that
    /// changed. The engine decodes its pictures anew, the one shown first.
    pub fn follow_gpu(&mut self, gpu: bool) -> bool {
        if self.gpu == gpu {
            return false;
        }
        self.gpu = gpu;
        if let Some(e) = &self.engine {
            e.set_hardware(gpu);
        }
        true
    }

    /// Hears `track`'s fader at `volume_db` while it plays, the plan
    /// unchanged. `None`: nothing plays here; `Some(false)`: the track has
    /// no sound in the plan.
    pub fn preview_fader(&self, track: &str, volume_db: f64) -> Option<bool> {
        let engine = self.engine.as_ref()?;
        let mut found = false;
        for (lane, t) in self.lanes.iter().enumerate() {
            if t.track == track {
                engine.set_lane_gain(lane, volume_db - t.volume_db);
                found = true;
            }
        }
        Some(found)
    }

    /// The motion to ask the renderer to present, when the latest one isn't
    /// on screen and the engine has its picture (or plays). Asked again if
    /// the renderer doesn't answer.
    pub fn present_due(&mut self, now: Instant) -> Option<u64> {
        if self.shown >= self.motion {
            return None;
        }
        if let Some((m, at)) = self.presenting
            && m == self.motion
            && now.duration_since(at) < PRESENT_AGAIN
        {
            return None;
        }
        let status = self.status()?;
        if !status.ready && !status.playing {
            return None;
        }
        self.presenting = Some((self.motion, now));
        Some(self.motion)
    }

    /// The renderer presented this player after motion `motion` was asked,
    /// showing `frame` exactly (`None`: the frame's picture wasn't there).
    pub fn presented(&mut self, motion: u64, frame: Option<i64>) {
        if self.presenting.is_some_and(|(m, _)| m == motion) {
            self.presenting = None;
        }
        let Some(frame) = frame else { return };
        if motion != self.motion {
            return;
        }
        if self.playing() || frame == self.target {
            self.shown = self.shown.max(motion);
        }
    }

    /// The picture of the frame the latest motion asked for failed to
    /// decode: why.
    pub fn failure(&self) -> Option<String> {
        let s = self.status()?;
        (s.failed.is_some() && !s.ready && !s.playing).then(|| s.error.unwrap_or_default())
    }

    /// Since the player was made: presents that showed an older picture
    /// because the clock's frame wasn't decoded yet (playing), and frames
    /// the clock passed between two presents without being shown.
    pub fn misses(&self) -> (u64, u64) {
        self.status().map_or((0, 0), |s| (s.late, s.skipped))
    }

    /// While playing: when the clock moves to another frame.
    pub fn next_change(&self, now: Instant) -> Option<Duration> {
        self.engine.as_ref()?.next_change(now)
    }

    pub fn snap(&self) -> PlayerSnap {
        let status = self.status();
        PlayerSnap {
            loaded: self.is_loaded(),
            muted: self.muted,
            frames: self.frames,
            position: self.position(),
            playing: status.as_ref().is_some_and(|s| s.playing),
            speed: status.as_ref().map_or(0.0, |s| s.speed),
            hwdec: hwdec(status.as_ref().and_then(|s| s.decoder)).into(),
            error: self.error.clone().or_else(|| status.and_then(|s| s.error)),
        }
    }
}

/// `player.state`'s `hwdec`: `nvdec` while the GPU decodes the picture
/// shown, `no` while the CPU does; empty before any.
fn hwdec(decoder: Option<Decoder>) -> &'static str {
    match decoder {
        Some(Decoder::Nvdec) => "nvdec",
        Some(Decoder::Cpu) => "no",
        None => "",
    }
}

/// Adds a frame of black and silence after `plan`'s content: the playhead
/// may stand at the end, after the last frame, where an edit adds clips.
fn with_end(plan: &mut RenderPlan) {
    let end = plan.range.1 - plan.range.0;
    let samples = plan.fps.frame_to_sample(end + 1, plan.sample_rate)
        - plan.fps.frame_to_sample(end, plan.sample_rate);
    match plan.video.last_mut() {
        Some(VideoPiece::Black { frames }) => *frames += 1,
        _ => plan.video.push(VideoPiece::Black { frames: 1 }),
    }
    for lane in &mut plan.audio {
        lane.push(AudioPiece::Silence { samples });
    }
    plan.frames += 1;
    plan.range.1 += 1;
}

/// `VIDCUT_DEBUG_PLAYER=1` traces what the players and the viewer do to
/// stderr.
pub fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VIDCUT_DEBUG_PLAYER").is_some())
}

#[cfg(test)]
mod tests {
    use vidcut_core::render::{AudioPiece, RenderPlan, VideoPiece};
    use vidcut_core::time::Rational;

    use super::with_end;

    #[test]
    fn the_end_is_a_frame_of_black_and_silence() {
        let fps = Rational::new(30000, 1001).unwrap();
        let mut plan = RenderPlan {
            fps,
            width: 64,
            height: 36,
            sample_rate: 48_000,
            channels: 2,
            frames: 3,
            video: vec![VideoPiece::Black { frames: 3 }],
            audio: vec![vec![AudioPiece::Silence { samples: 4804 }]],
            range: (0, 3),
            name: "t".into(),
        };
        with_end(&mut plan);
        assert_eq!((plan.frames, plan.range), (4, (0, 4)));
        assert_eq!(plan.video, vec![VideoPiece::Black { frames: 4 }]);
        // Frame 3's samples, as the plan counts them.
        let samples = fps.frame_to_sample(4, 48_000) - fps.frame_to_sample(3, 48_000);
        assert_eq!(plan.audio[0][1], AudioPiece::Silence { samples });
    }
}
