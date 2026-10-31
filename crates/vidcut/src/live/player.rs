use std::sync::Arc;
use std::time::{Duration, Instant};

use vidcut_av::player::{self as engine, AudioOutput, Decoder, PacedOutput, PlaysOn};
use vidcut_core::model::Title;
use vidcut_core::render::{AudioPiece, LaneTrack, RenderPlan, VideoPiece};

use super::Renderer;
use super::snapshot::{PlayerId, PlayerSnap};

pub type MakeOutput = Arc<dyn Fn() -> Box<dyn AudioOutput> + Send + Sync>;

const PRESENT_AGAIN: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy)]
struct Presenting {
    motion: u64,
    at: Instant,
    position: i64,
    answered: bool,
}

#[derive(Default)]
struct Content {
    plan: Option<RenderPlan>,
    lanes: Vec<LaneTrack>,
    faders: std::collections::BTreeMap<usize, f64>,
    frames: i64,
    position: i64,
    decoder: Option<Decoder>,
    error: Option<String>,
}

pub struct Player {
    output: Option<MakeOutput>,
    renderer: Arc<dyn Renderer>,
    on_picture: Arc<dyn Fn() + Send + Sync>,
    gpu_pictures: vidcut_av::hw::GpuPictures,
    engine: Option<Arc<engine::Player>>,
    plays: Option<PlayerId>,
    format: (u32, u16),
    active: PlayerId,
    contents: [Content; 2],
    pub target: i64,
    pub motion: u64,
    pub shown: u64,
    presenting: Option<Presenting>,
    landed: bool,
    auditioning: bool,
    skips: Option<Vec<(i64, i64)>>,
    looping: (bool, Option<(i64, i64)>),
    muted: bool,
    scrubbing: bool,
    decoding: crate::preferences::GpuDecoding,
    audio_device: Option<String>,
    plays_on: PlaysOn,
    pub audio_error: Option<String>,
    subtitles: Option<engine::SubtitleView>,
}

impl Player {
    pub fn new(
        output: Option<MakeOutput>,
        gpu_pictures: vidcut_av::hw::GpuPictures,
        renderer: Arc<dyn Renderer>,
        on_picture: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let decoding = crate::preferences::gpu_decoding();
        vidcut_av::debug!("decoding", "the player starts with {decoding}");
        Self {
            output,
            renderer,
            on_picture,
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
            landed: false,
            auditioning: false,
            skips: None,
            looping: (false, None),
            muted: false,
            scrubbing: true,
            decoding,
            audio_device: crate::preferences::audio_device(),
            plays_on: PlaysOn::Default,
            audio_error: None,
            subtitles: None,
        }
    }

    pub fn is_loaded(&self, id: PlayerId) -> bool {
        self.contents[id.index()].plan.is_some()
    }

    pub fn preview_pictures(&self, pictures: Vec<(String, vidcut_core::model::Picture)>) {
        if let Some(e) = &self.engine {
            e.preview_pictures(pictures);
        }
    }

    pub fn preview_title(&self, title: Option<(String, Title)>) {
        if let Some(e) = &self.engine {
            e.preview_title(title);
        }
    }

    pub fn set_subtitles(&mut self, view: Option<engine::SubtitleView>) {
        self.subtitles = view;
        self.show_subtitles();
    }

    fn show_subtitles(&self) {
        if let Some(e) = &self.engine {
            let on = self.plays == Some(PlayerId::Timeline);
            e.set_subtitles(self.subtitles.clone().filter(|_| on));
        }
    }

    fn shown_engine(&self) -> Option<&Arc<engine::Player>> {
        self.engine
            .as_ref()
            .filter(|_| self.plays == Some(self.active))
    }

    fn engine_of(&self, id: PlayerId) -> Option<&Arc<engine::Player>> {
        self.shown_engine().filter(|_| id == self.active)
    }

    fn status(&self) -> Option<engine::Status> {
        self.shown_engine().map(|e| e.status())
    }

    pub fn position(&self, id: PlayerId) -> i64 {
        let c = &self.contents[id.index()];
        if let (Some(skips), Some(e)) = (&self.skips, self.engine_of(id)) {
            return unskipped(skips, e.status().position).clamp(0, c.frames.max(0));
        }
        match self.engine_of(id).filter(|_| !self.auditioning) {
            Some(e) => e.status().position.clamp(0, c.frames.max(0)),
            None => c.position,
        }
    }

    pub fn stats(
        &self,
        viewer: Option<super::stats::ViewerStats>,
    ) -> Option<super::stats::PlayerStats> {
        self.shown_engine()
            .map(|e| super::stats::PlayerStats::of(e, viewer))
    }

    pub fn playing(&self) -> bool {
        self.status().is_some_and(|s| s.playing)
    }

    fn instant(&mut self) -> u64 {
        self.motion += 1;
        self.shown = self.motion;
        self.motion
    }

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
            self.contents[i].frames = frames;
            return None;
        }
        with_end(&mut plan);
        if self.auditioning && id == PlayerId::Timeline {
            let c = &mut self.contents[i];
            c.position = at.unwrap_or(c.position).clamp(0, frames);
            c.plan = Some(plan);
            c.frames = frames;
            c.lanes = lanes;
            c.faders.clear();
            c.error = None;
            return None;
        }
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
        self.show_subtitles();
        if let Some(e) = &self.engine {
            for (&lane, &db) in &self.contents[id.index()].faders {
                e.set_lane_gain(lane, db);
            }
        }
        self.prepare_other();
        self.motion += 1;
        Some(self.motion)
    }

    fn open(&mut self, plan: RenderPlan) -> Option<Arc<engine::Player>> {
        let make = self.output.clone()?;
        if self.engine.take().is_some() {
            self.plays = None;
            self.renderer.attach(None);
        }
        let config = || engine::Config {
            part: 1,
            audio_scrubbing: false,
            on_picture: Some(self.on_picture.clone()),
            hardware: self.decoding.on,
            gpu_pictures: self.gpu_pictures.clone(),
        };
        self.audio_error = None;
        match engine::Player::new(plan.clone(), config(), make()) {
            Ok(e) => {
                self.plays_on = e.plays_on();
                if let PlaysOn::Missing(id) = &self.plays_on {
                    self.audio_error = Some(format!(
                        "the audio device {id} isn't connected: the system's default plays"
                    ));
                }
                return Some(Arc::new(e));
            }
            Err(e) => self.audio_error = Some(e.to_string()),
        }
        self.plays_on = PlaysOn::Default;
        let silent = Box::new(PacedOutput::new(Duration::from_millis(10), Duration::ZERO));
        match engine::Player::new(plan, config(), silent) {
            Ok(e) => Some(Arc::new(e)),
            Err(e) => {
                self.contents[self.active.index()].error = Some(e.to_string());
                None
            }
        }
    }

    fn prepare_other(&self) {
        let Some(engine) = &self.engine else { return };
        let other = &self.contents[self.active.other().index()];
        engine.prepare(other.plan.clone().map(|p| (p, other.position)));
    }

    pub fn audition(
        &mut self,
        mut plan: RenderPlan,
        at: i64,
        speed: f64,
        skips: Option<Vec<(i64, i64)>>,
    ) -> Option<u64> {
        if self.active != PlayerId::Timeline || (plan.sample_rate, plan.channels) != self.format {
            return None;
        }
        let engine = self.shown_engine().cloned()?;
        if !self.auditioning {
            let c = &mut self.contents[PlayerId::Timeline.index()];
            c.position = self.target.clamp(0, c.frames.max(0));
            self.auditioning = true;
        }
        with_end(&mut plan);
        let at = at.clamp(0, plan.frames - 1);
        self.skips = skips;
        if engine.set_plan_at(plan, at).is_err() {
            return self.end_audition();
        }
        engine.set_loop(None);
        engine.play(speed);
        self.motion += 1;
        Some(self.motion)
    }

    pub fn skipping(&self) -> bool {
        self.skips.is_some()
    }

    pub fn auditioning(&self) -> bool {
        self.auditioning
    }

    pub fn end_audition(&mut self) -> Option<u64> {
        if !std::mem::take(&mut self.auditioning) {
            return None;
        }
        if let Some(e) = self.shown_engine() {
            e.pause();
        }
        if let Some(skips) = self.skips.take()
            && let Some(at) = self.shown_engine().map(|e| e.status().position)
        {
            let c = &mut self.contents[PlayerId::Timeline.index()];
            c.position = unskipped(&skips, at).clamp(0, c.frames.max(0));
        }
        let at = self.contents[PlayerId::Timeline.index()].position;
        let shown = self.play_shown(Some(at));
        self.apply_loop();
        shown
    }

    pub fn show(&mut self, id: PlayerId) -> Option<u64> {
        self.end_audition();
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
        self.plays = None;
        self.renderer.attach(None);
        self.prepare_other();
        self.target = self.contents[id.index()].position;
        Some(self.instant())
    }

    pub fn unload(&mut self, id: PlayerId) {
        if id == PlayerId::Timeline {
            self.auditioning = false;
            self.skips = None;
        }
        self.contents[id.index()] = Content::default();
        if !self.is_loaded(id.other()) {
            if self.engine.take().is_some() {
                self.renderer.attach(None);
            }
            self.plays = None;
        } else if id == self.active {
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

    pub fn seek(&mut self, id: PlayerId, frame: i64, sound: bool) -> Option<u64> {
        self.end_audition();
        if id != self.active {
            let c = &mut self.contents[id.index()];
            c.position = match c.plan {
                Some(_) => frame.clamp(0, c.frames.max(0)),
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

    pub fn step(&mut self, frames: i64) -> u64 {
        self.end_audition();
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

    pub fn play(&mut self, speed: f64) {
        self.end_audition();
        if let Some(e) = self.shown_engine() {
            e.play(speed);
        }
    }

    pub fn pause(&mut self) {
        self.end_audition();
        if let Some(e) = self.shown_engine() {
            e.pause();
        }
    }

    pub fn set_loop(&mut self, on: bool, marks: Option<(i64, i64)>) {
        self.looping = (on, marks);
        if !self.auditioning {
            self.apply_loop();
        }
    }

    fn apply_loop(&self) {
        if let Some(e) = self.shown_engine() {
            let (on, marks) = self.looping;
            let frames = self.contents[self.active.index()].frames;
            e.set_loop(on.then(|| marks.unwrap_or((0, frames))));
        }
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        if let Some(e) = &self.engine {
            e.set_muted(muted);
        }
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    pub fn set_scrubbing(&mut self, on: bool) {
        self.scrubbing = on;
        if let Some(e) = &self.engine {
            e.set_audio_scrubbing(on);
        }
    }

    pub fn follow_decoding(&mut self, decoding: crate::preferences::GpuDecoding) -> bool {
        if self.decoding == decoding {
            return false;
        }
        let other_gpu = self.decoding.gpu != decoding.gpu;
        vidcut_av::debug!(
            "decoding",
            "the preferences changed: {decoding}; the player {}",
            if other_gpu {
                "opens anew on the GPU in use"
            } else {
                "decodes its pictures anew"
            }
        );
        self.decoding = decoding;
        if other_gpu {
            self.reopen();
        } else if let Some(e) = &self.engine {
            e.set_hardware(self.decoding.on);
        }
        true
    }

    pub fn audio_device(&self) -> Option<&str> {
        match &self.plays_on {
            PlaysOn::Chosen(id) => Some(id),
            _ => None,
        }
    }

    pub fn audio_notice(&self) -> Option<String> {
        let e = self.audio_error.as_ref()?;
        Some(match self.plays_on {
            PlaysOn::Missing(_) => e.clone(),
            _ => format!("no sound: {e}"),
        })
    }

    pub fn follow_audio(&mut self, device: Option<String>) -> bool {
        if self.audio_device == device {
            return false;
        }
        self.audio_device = device;
        self.reopen();
        true
    }

    fn reopen(&mut self) {
        if let Some(e) = self.engine.take() {
            if let Some(id) = self.plays {
                let c = &mut self.contents[id.index()];
                c.position = e.status().position.clamp(0, c.frames.max(0));
            }
            self.plays = None;
            self.renderer.attach(None);
            drop(e);
            self.play_shown(None);
        }
    }

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

    pub fn present_due(&mut self, now: Instant) -> Option<u64> {
        if self.shown >= self.motion {
            return None;
        }
        let asked = self.presenting.filter(|p| p.motion == self.motion);
        if asked.is_some_and(|p| !p.answered && now.duration_since(p.at) < PRESENT_AGAIN) {
            return None;
        }
        let status = self.status()?;
        if !status.ready && !status.playing {
            return None;
        }
        if asked.is_some_and(|p| p.answered && p.position == status.position) && !self.landed {
            return None;
        }
        self.landed = false;
        self.presenting = Some(Presenting {
            motion: self.motion,
            at: now,
            position: status.position,
            answered: false,
        });
        Some(self.motion)
    }

    pub fn picture_landed(&mut self) {
        self.landed = true;
    }

    pub fn presented(&mut self, motion: u64, frame: Option<i64>) {
        if let Some(p) = self.presenting.as_mut()
            && p.motion == motion
        {
            p.answered = true;
        }
        let Some(frame) = frame else { return };
        if motion != self.motion {
            return;
        }
        if self.playing() || frame == self.target {
            self.shown = self.shown.max(motion);
        }
    }

    pub fn failure(&self) -> Option<String> {
        let s = self.status()?;
        (s.failed.is_some() && !s.ready && !s.playing).then(|| s.error.unwrap_or_default())
    }

    pub fn error(&self) -> Option<&String> {
        self.contents[self.active.index()].error.as_ref()
    }

    pub fn drop_counts(&self, id: PlayerId) -> Option<(f64, i64, super::drops::Counts)> {
        if self.skips.is_some() || self.auditioning {
            return None;
        }
        let s = self.engine_of(id)?.status();
        let counts = super::drops::Counts {
            late: s.late,
            skipped: s.skipped,
            frames: s.frames,
            making: s.making,
        };
        Some((if s.playing { s.speed } else { 0.0 }, s.position, counts))
    }

    pub fn misses(&self) -> (u64, u64) {
        self.engine.as_ref().map_or((0, 0), |e| {
            let s = e.status();
            (s.late, s.skipped)
        })
    }

    pub fn next_change(&self, now: Instant) -> Option<Duration> {
        self.shown_engine()?.next_change(now)
    }

    pub fn snap(&self, id: PlayerId) -> PlayerSnap {
        let c = &self.contents[id.index()];
        let status = self.engine_of(id).map(|e| e.status());
        let ready = match &status {
            Some(s) => s.ready,
            None if id != self.active => {
                c.plan.is_some() && self.engine.as_ref().is_some_and(|e| e.status().prepared)
            }
            None => false,
        };
        let (position, glide) = match (&status, &self.skips) {
            (Some(s), Some(skips)) => (
                unskipped(skips, s.position).clamp(0, c.frames.max(0)),
                s.glide,
            ),
            (Some(s), None) if !self.auditioning => (s.position.clamp(0, c.frames.max(0)), s.glide),
            _ => (c.position, None),
        };
        PlayerSnap {
            loaded: c.plan.is_some(),
            muted: self.muted,
            frames: c.frames,
            position,
            ready,
            playing: status.as_ref().is_some_and(|s| s.playing),
            speed: status.as_ref().map_or(0.0, |s| s.speed),
            hwdec: hwdec(status.as_ref().map_or(c.decoder, |s| s.decoder)).into(),
            error: c.error.clone().or_else(|| status.and_then(|s| s.error)),
            lanes: c.lanes.iter().map(|l| l.track.clone()).collect(),
            glide,
        }
    }
}

fn hwdec(decoder: Option<Decoder>) -> &'static str {
    match decoder {
        Some(Decoder::Cpu) => "no",
        Some(gpu) => gpu.name(),
        None => "",
    }
}

fn with_end(plan: &mut RenderPlan) {
    let end = plan.range.1 - plan.range.0;
    let samples = plan.fps.frame_to_sample(end + 1, plan.sample_rate)
        - plan.fps.frame_to_sample(end, plan.sample_rate);
    match plan.video.last_mut() {
        Some(VideoPiece::Black { frames, .. }) => *frames += 1,
        _ => plan.video.push(VideoPiece::black(1)),
    }
    for lane in &mut plan.audio {
        lane.push(AudioPiece::Silence { samples });
    }
    plan.frames += 1;
    plan.range.1 += 1;
}

pub fn skipped_to(skips: &[(i64, i64)], frame: i64) -> i64 {
    let mut out = frame;
    for &(a, b) in skips {
        if b <= frame {
            out -= b - a;
        } else if a <= frame {
            out -= frame - a;
        }
    }
    out
}

pub fn unskipped(skips: &[(i64, i64)], frame: i64) -> i64 {
    let mut out = frame;
    for &(a, b) in skips {
        if a > out {
            break;
        }
        out += b - a;
    }
    out
}
