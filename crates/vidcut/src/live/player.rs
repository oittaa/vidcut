//! The viewer's player as the live session drives it: vidcut's own engine
//! (`vidcut_av::player`, docs/decisions/player.md) playing the render plan
//! of what the viewer shows, the source or the timeline, where each one's
//! playhead is, and when a requested frame is on screen
//! (docs/decisions/gui.md §4).
//!
//! **One engine for both.** The one not shown keeps its plan and playhead
//! here, and the engine keeps the picture there ready (`prepare`): decoded
//! while nothing else is, held whatever the memory budget. Switching plays
//! the other plan from its playhead, so its picture shows at once, and
//! prepares the one left. An edit seen from the source (the timeline's
//! playhead moving after an edit function, decision D15) prepares the
//! timeline's new frame the same way.
//!
//! Every seek, step, load and switch of what is shown is a motion with a
//! number. A motion is on screen once the renderer presented the engine
//! after it and showed the frame it asked for exactly: paused, that frame;
//! playing, any frame (the clock starts at the frame asked for and moves
//! on from there). The session asks the renderer to present when the
//! engine has the frame's picture.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vidcut_av::player::{self as engine, AudioOutput, Decoder, PacedOutput};
use vidcut_core::render::{AudioPiece, LaneTrack, RenderPlan, VideoPiece};

use super::Renderer;
use super::snapshot::{PlayerId, PlayerSnap};

/// Makes the engine's audio output: the system's device in the GUI, a
/// stand-in in tests.
pub type MakeOutput = Arc<dyn Fn() -> Box<dyn AudioOutput> + Send + Sync>;

/// How long a present asked for may go unanswered before it is asked again.
const PRESENT_AGAIN: Duration = Duration::from_millis(50);

/// The source or the timeline, as the viewer would show it.
#[derive(Default)]
struct Content {
    /// The plan: proxies swapped in, and a frame of black after the
    /// content, where the playhead stands at its end.
    plan: Option<RenderPlan>,
    /// The tracks of the plan's audio lanes (the timeline's), for faders
    /// moved while it plays.
    lanes: Vec<LaneTrack>,
    /// Gains on top of lanes while their faders are dragged
    /// (`mixer.preview`), dB: heard again after a switch away and back. A
    /// new plan has the saved levels and drops them.
    faders: std::collections::BTreeMap<usize, f64>,
    /// Length of the content in frames (the playhead goes up to it).
    frames: i64,
    /// The playhead while the engine plays something else (remembered for
    /// the next load while nothing is loaded).
    position: i64,
    /// What decoded the picture shown last while it was shown.
    decoder: Option<Decoder>,
    error: Option<String>,
}

pub struct Player {
    /// Makes the engine's audio output; `None`: no preview at all.
    output: Option<MakeOutput>,
    renderer: Arc<dyn Renderer>,
    /// Called on the engine's decode thread when the picture the viewer is
    /// at lands (or fails).
    on_picture: Arc<dyn Fn() + Send + Sync>,
    /// Memory for the engine's pictures.
    budget: engine::Budget,
    /// The renderer shows pictures NVDEC decoded from GPU memory: they stay
    /// there.
    gpu_pictures: bool,
    engine: Option<Arc<engine::Player>>,
    /// Whose plan the engine plays. The renderer shows the engine when
    /// that is the one shown.
    plays: Option<PlayerId>,
    /// The sample rate and channels the engine's device was opened for.
    format: (u32, u16),
    /// What the viewer shows.
    active: PlayerId,
    contents: [Content; 2],
    /// The frame the latest seek, step, load or switch asked for. Playback
    /// leaves it alone, so a waiting seek is overtaken only by another
    /// motion.
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
    /// The audio device failed to open: the engine plays to a stand-in.
    pub audio_error: Option<String>,
}

impl Player {
    pub fn new(
        output: Option<MakeOutput>,
        budget: engine::Budget,
        gpu_pictures: bool,
        renderer: Arc<dyn Renderer>,
        on_picture: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            output,
            renderer,
            on_picture,
            budget,
            gpu_pictures,
            engine: None,
            plays: None,
            format: (0, 0),
            active: PlayerId::Timeline,
            contents: Default::default(),
            target: 0,
            motion: 0,
            shown: 0,
            presenting: None,
            muted: false,
            scrubbing: true,
            gpu: crate::preferences::gpu_acceleration(),
            audio_error: None,
        }
    }

    pub fn is_loaded(&self, id: PlayerId) -> bool {
        self.contents[id.index()].plan.is_some()
    }

    /// The engine, while it plays what the viewer shows.
    fn shown_engine(&self) -> Option<&Arc<engine::Player>> {
        self.engine
            .as_ref()
            .filter(|_| self.plays == Some(self.active))
    }

    /// The engine playing `id`, while the viewer shows it.
    fn engine_of(&self, id: PlayerId) -> Option<&Arc<engine::Player>> {
        self.shown_engine().filter(|_| id == self.active)
    }

    fn status(&self) -> Option<engine::Status> {
        self.shown_engine().map(|e| e.status())
    }

    /// `id`'s playhead: where the engine is while it is shown (the clock
    /// while playing).
    pub fn position(&self, id: PlayerId) -> i64 {
        let c = &self.contents[id.index()];
        match self.engine_of(id) {
            Some(e) => e.status().position.clamp(0, c.frames.max(0)),
            None => c.position,
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

    /// Gives `id` the plan `plan` (its content `plan.frames` long), with
    /// `lanes` its audio lanes' tracks. `at`: the frame to show (clamped to
    /// the content); `None` stays where its playhead is, playing on if it
    /// plays. Returns the motion when the viewer shows `id`, or `None` when
    /// the picture can't have changed or `id` isn't shown (its frame is
    /// then prepared).
    pub fn load(
        &mut self,
        id: PlayerId,
        mut plan: RenderPlan,
        lanes: Vec<LaneTrack>,
        at: Option<i64>,
    ) -> Option<u64> {
        let frames = plan.frames;
        let i = id.index();
        if self.output.is_none() {
            // No preview: nothing loads.
            self.contents[i].frames = frames;
            return None;
        }
        with_end(&mut plan);
        if self.contents[i].plan.as_ref() == Some(&plan) {
            self.contents[i].frames = frames;
            let to = at?.clamp(0, frames);
            if to == self.position(id) && (id != self.active || to == self.target) {
                return None;
            }
            return self.seek(id, to, false);
        }
        let c = &mut self.contents[i];
        c.plan = Some(plan);
        c.frames = frames;
        c.lanes = lanes;
        c.faders.clear();
        c.error = None;
        if id != self.active {
            c.position = at.unwrap_or(c.position).clamp(0, frames);
            self.prepare_other();
            return None;
        }
        self.play_shown(at)
    }

    /// The engine plays the plan of what the viewer shows: from `at`, or
    /// from its playhead (where it is, playing on if it plays, when the
    /// engine played it already). A new engine when there is none, or its
    /// device was opened for another sample rate or channel count. Then
    /// the other's frame is prepared. Returns the motion.
    fn play_shown(&mut self, at: Option<i64>) -> Option<u64> {
        let id = self.active;
        let c = &self.contents[id.index()];
        let plan = c.plan.clone()?;
        let (frames, position) = (c.frames, c.position.clamp(0, c.frames));
        let at = at.map(|f| f.clamp(0, frames));
        let format = (plan.sample_rate, plan.channels);
        let engine = self.engine.clone().filter(|_| self.format == format);
        let target = match engine {
            Some(e) => {
                let result = match (self.plays == Some(id), at) {
                    (true, None) => e.set_plan(plan),
                    (true, Some(f)) => e.set_plan_at(plan, f),
                    (false, _) => e.set_plan_at(plan, at.unwrap_or(position)),
                };
                if let Err(err) = result {
                    self.contents[id.index()].error = Some(err.to_string());
                    return None;
                }
                match (self.plays == Some(id), at) {
                    (true, None) => self.target.clamp(0, frames),
                    (_, Some(f)) => f,
                    (false, None) => position,
                }
            }
            None => {
                let from = at.unwrap_or(position);
                let Some(engine) = self.open(plan.clone()) else {
                    // Nothing shows: loaded again next time.
                    self.contents[id.index()].plan = None;
                    return None;
                };
                engine.seek_quietly(from);
                engine.set_audio_scrubbing(self.scrubbing);
                engine.set_muted(self.muted);
                self.engine = Some(engine);
                self.format = format;
                from
            }
        };
        self.target = target;
        if self.plays != Some(id) {
            self.plays = Some(id);
            self.renderer.attach(self.engine.clone());
        }
        // A new plan starts without gains on top: faders still held again.
        if let Some(e) = &self.engine {
            for (&lane, &db) in &self.contents[id.index()].faders {
                e.set_lane_gain(lane, db);
            }
        }
        self.prepare_other();
        self.motion += 1;
        Some(self.motion)
    }

    /// A new engine for `plan`, paused at its first frame. Its sound goes to
    /// the output made for it, or to a silent stand-in when that won't open
    /// (no audio device): the picture plays all the same. The engine it
    /// replaces is dropped first, so the device is free again.
    fn open(&mut self, plan: RenderPlan) -> Option<Arc<engine::Player>> {
        let make = self.output.clone()?;
        if self.engine.take().is_some() {
            self.plays = None;
            self.renderer.attach(None);
        }
        let config = || engine::Config {
            budget: self.budget,
            // Moves the session makes by itself play no sound; on after.
            audio_scrubbing: false,
            on_picture: Some(self.on_picture.clone()),
            hardware: self.gpu,
            gpu_pictures: self.gpu_pictures,
        };
        self.audio_error = None;
        match engine::Player::new(plan.clone(), config(), make()) {
            Ok(e) => return Some(Arc::new(e)),
            Err(e) => self.audio_error = Some(e.to_string()),
        }
        let silent = Box::new(PacedOutput::new(Duration::from_millis(10), Duration::ZERO));
        match engine::Player::new(plan, config(), silent) {
            Ok(e) => Some(Arc::new(e)),
            Err(e) => {
                self.contents[self.active.index()].error = Some(e.to_string());
                None
            }
        }
    }

    /// The engine keeps the picture at the playhead of what isn't shown
    /// ready, or none when nothing is loaded there.
    fn prepare_other(&self) {
        let Some(engine) = &self.engine else { return };
        let other = &self.contents[self.active.other().index()];
        engine.prepare(other.plan.clone().map(|p| (p, other.position)));
    }

    /// Shows `id` instead of the other: the engine plays its plan from its
    /// playhead, paused (the picture there was kept ready), and prepares
    /// the frame of the one left. Returns the motion, or `None` when `id`
    /// is shown already.
    pub fn show(&mut self, id: PlayerId) -> Option<u64> {
        if id == self.active {
            return None;
        }
        let left = self.active;
        if let Some(e) = self.shown_engine().cloned() {
            e.pause();
            let status = e.status();
            let c = &mut self.contents[left.index()];
            c.position = status.position.clamp(0, c.frames.max(0));
            c.decoder = status.decoder;
        }
        self.active = id;
        if self.is_loaded(id) {
            return self.play_shown(None);
        }
        // Nothing to show: black, the frame left prepared.
        self.plays = None;
        self.renderer.attach(None);
        self.prepare_other();
        self.target = self.contents[id.index()].position;
        Some(self.instant())
    }

    /// `id` shows nothing. With nothing loaded either, the engine goes.
    pub fn unload(&mut self, id: PlayerId) {
        self.contents[id.index()] = Content::default();
        if !self.is_loaded(id.other()) {
            if self.engine.take().is_some() {
                self.renderer.attach(None);
            }
            self.plays = None;
        } else if id == self.active {
            // The engine plays what no one shows.
            if let Some(e) = self.shown_engine() {
                e.pause();
            }
            self.plays = None;
            self.renderer.attach(None);
        } else {
            self.prepare_other();
            return;
        }
        self.target = 0;
        self.instant();
    }

    /// Moves `id`'s playhead to `frame` (clamped to the content). Paused,
    /// the frame's sound plays when `sound` and audio scrubbing are on (a
    /// drag of the playhead is seeks); playing, play goes on from there.
    /// Returns the motion when the viewer shows `id`; otherwise its new
    /// frame is prepared.
    pub fn seek(&mut self, id: PlayerId, frame: i64, sound: bool) -> Option<u64> {
        if id != self.active {
            let c = &mut self.contents[id.index()];
            c.position = match c.plan {
                Some(_) => frame.clamp(0, c.frames.max(0)),
                // Remembered for the next load (which clamps it).
                None => frame.max(0),
            };
            self.prepare_other();
            return None;
        }
        let Some(engine) = self.shown_engine().cloned() else {
            let c = &mut self.contents[id.index()];
            c.position = frame.max(0);
            self.target = c.position;
            return Some(self.instant());
        };
        let frame = frame.clamp(0, self.contents[id.index()].frames.max(0));
        self.target = frame;
        if sound {
            engine.seek(frame);
        } else {
            engine.seek_quietly(frame);
        }
        self.motion += 1;
        Some(self.motion)
    }

    /// Pauses what the viewer shows and moves `frames` from where it is,
    /// playing the frame's sound when scrubbing. Returns the motion.
    pub fn step(&mut self, frames: i64) -> u64 {
        let Some(engine) = self.shown_engine().cloned() else {
            let c = &self.contents[self.active.index()];
            let to = c.position.saturating_add(frames).max(0);
            return self.seek(self.active, to, false).unwrap_or(self.motion);
        };
        engine.step(frames);
        self.target = engine.status().position;
        self.motion += 1;
        self.motion
    }

    /// Plays what the viewer shows at `speed` (negative: backward, the
    /// sound reversed).
    pub fn play(&mut self, speed: f64) {
        if let Some(e) = self.shown_engine() {
            e.play(speed);
        }
    }

    /// Stops where the viewer is.
    pub fn pause(&mut self) {
        if let Some(e) = self.shown_engine() {
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

    /// Keeps pictures within `budget` from now on (the preferences'
    /// `player.memory_mb` and `video_memory_mb`).
    pub fn set_budget(&mut self, budget: engine::Budget) {
        self.budget = budget;
        if let Some(e) = &self.engine {
            e.set_budget(budget);
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

    /// Hears `track`'s fader at `volume_db` while the timeline plays, the
    /// plan unchanged; held while the source is shown, and heard again
    /// when the timeline is. `None`: the viewer doesn't play the timeline;
    /// `Some(false)`: the track has no sound in the plan.
    pub fn preview_fader(&mut self, track: &str, volume_db: f64) -> Option<bool> {
        let c = &mut self.contents[PlayerId::Timeline.index()];
        let mut found = false;
        for (lane, t) in c.lanes.iter().enumerate() {
            if t.track == track {
                c.faders.insert(lane, volume_db - t.volume_db);
                found = true;
            }
        }
        let engine = self.engine_of(PlayerId::Timeline)?;
        for (&lane, &db) in &self.contents[PlayerId::Timeline.index()].faders {
            engine.set_lane_gain(lane, db);
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

    /// The renderer presented the engine after motion `motion` was asked,
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

    /// Why what the viewer shows can't play, if so.
    pub fn error(&self) -> Option<&String> {
        self.contents[self.active.index()].error.as_ref()
    }

    /// Since the engine was made: presents that showed an older picture
    /// because the clock's frame wasn't decoded yet (playing), and frames
    /// the clock passed between two presents without being shown.
    pub fn misses(&self) -> (u64, u64) {
        self.engine.as_ref().map_or((0, 0), |e| {
            let s = e.status();
            (s.late, s.skipped)
        })
    }

    /// While playing: when the clock moves to another frame.
    pub fn next_change(&self, now: Instant) -> Option<Duration> {
        self.shown_engine()?.next_change(now)
    }

    pub fn snap(&self, id: PlayerId) -> PlayerSnap {
        let c = &self.contents[id.index()];
        let status = self.engine_of(id).map(|e| e.status());
        let ready = match &status {
            Some(s) => s.ready,
            // Not shown: its frame is the one prepared.
            None if id != self.active => {
                c.plan.is_some() && self.engine.as_ref().is_some_and(|e| e.status().prepared)
            }
            None => false,
        };
        PlayerSnap {
            loaded: c.plan.is_some(),
            muted: self.muted,
            frames: c.frames,
            position: self.position(id),
            ready,
            playing: status.as_ref().is_some_and(|s| s.playing),
            speed: status.as_ref().map_or(0.0, |s| s.speed),
            hwdec: hwdec(status.as_ref().map_or(c.decoder, |s| s.decoder)).into(),
            error: c.error.clone().or_else(|| status.and_then(|s| s.error)),
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

/// `VIDCUT_DEBUG_PLAYER=1` traces what the player and the viewer do to
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
