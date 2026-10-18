use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::clock::Heard;
use super::ring::Ring;
use crate::error::{Error, Result};

#[derive(Clone)]
pub struct AudioSource {
    pub(crate) ring: Arc<Ring>,
    pub(crate) heard: Arc<Heard>,
    rate: u32,
    channels: u16,
    pub(crate) failure: Arc<Mutex<Option<String>>>,
}

impl AudioSource {
    pub(crate) fn new(ring: Arc<Ring>, heard: Arc<Heard>, rate: u32, channels: u16) -> Self {
        Self {
            ring,
            heard,
            rate,
            channels,
            failure: Arc::new(Mutex::new(None)),
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn fill(&self, out: &mut [f32], heard_at: Instant) {
        let (counter, n) = self.ring.pop(out);
        out[n * self.ring.channels()..].fill(0.0);
        self.heard.record(counter, n as u64, heard_at);
    }

    pub fn fail(&self, why: String) {
        *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
    }

    fn recover(&self) {
        *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

pub trait AudioOutput: Send {
    fn start(&mut self, source: AudioSource) -> Result<()>;

    fn plays_on(&self) -> PlaysOn {
        PlaysOn::Default
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PlaysOn {
    #[default]
    Default,
    Chosen(String),
    Missing(String),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OutputDevice {
    pub id: String,
    pub name: String,
    pub default: bool,
}

pub fn output_devices() -> Result<Vec<OutputDevice>> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let default = host.default_output_device().and_then(|d| d.id().ok());
    let devices = host
        .output_devices()
        .map_err(|e| Error::Invalid(format!("listing audio devices: {e}")))?;
    let (mut listed, mut details) = (Vec::new(), Vec::new());
    for d in devices {
        let Ok(id) = d.id() else { continue };
        listed.push(OutputDevice {
            default: default.as_ref() == Some(&id),
            id: id.to_string(),
            name: d.to_string(),
        });
        details.push(
            d.description()
                .ok()
                .and_then(|desc| desc.extended().nth(1).map(str::to_string)),
        );
    }
    without_numbered_cards(&mut listed, &mut details);
    one_for_each_output(&mut listed, &mut details);
    distinct_names(&mut listed, &details);
    Ok(listed)
}

fn one_for_each_output(devices: &mut Vec<OutputDevice>, details: &mut Vec<Option<String>>) {
    fn plugin(id: &str) -> Option<&str> {
        id.strip_prefix("alsa:")
            .map(|rest| rest.split(':').next().unwrap_or(rest))
    }
    fn card(id: &str) -> Option<String> {
        id.split("CARD=")
            .nth(1)
            .map(|c| c.split(',').next().unwrap_or(c).to_string())
    }
    fn listed(id: &str) -> bool {
        matches!(
            plugin(id),
            Some("default" | "sysdefault" | "pipewire" | "pulse" | "hdmi")
        )
    }
    let covered: std::collections::HashSet<String> = devices
        .iter()
        .filter(|d| listed(&d.id))
        .filter_map(|d| card(&d.id))
        .collect();
    let keep: Vec<bool> = devices
        .iter()
        .map(|d| match plugin(&d.id) {
            None => true,
            Some(p) => {
                listed(&d.id) || (p == "hw" && card(&d.id).is_some_and(|c| !covered.contains(&c)))
            }
        })
        .collect();
    let mut kept = keep.iter();
    devices.retain(|_| *kept.next().expect("one for each"));
    let mut kept = keep.iter();
    details.retain(|_| *kept.next().expect("one for each"));
}

fn without_numbered_cards(devices: &mut Vec<OutputDevice>, details: &mut Vec<Option<String>>) {
    let numbered = |id: &str| {
        id.split("CARD=")
            .nth(1)
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| c.is_ascii_digit())
    };
    let keep: Vec<bool> = (0..devices.len())
        .map(|i| {
            let twin = |j: usize| {
                j != i
                    && !numbered(&devices[j].id)
                    && devices[j].name == devices[i].name
                    && details[j] == details[i]
            };
            !(numbered(&devices[i].id) && (0..devices.len()).any(twin))
        })
        .collect();
    let mut kept = keep.iter();
    devices.retain(|_| *kept.next().expect("one for each"));
    let mut kept = keep.iter();
    details.retain(|_| *kept.next().expect("one for each"));
}

fn distinct_names(devices: &mut [OutputDevice], details: &[Option<String>]) {
    let repeats = |devices: &[OutputDevice], name: &str| {
        devices.iter().filter(|d| d.name == name).count() > 1
    };
    let repeated: Vec<bool> = devices.iter().map(|d| repeats(devices, &d.name)).collect();
    for ((d, detail), repeated) in devices.iter_mut().zip(details).zip(repeated) {
        if let Some(detail) = detail.as_deref().filter(|_| repeated) {
            d.name = format!("{} ({detail})", d.name);
        }
    }
    for i in 0..devices.len() {
        let name = devices[i].name.clone();
        if repeats(devices, &name) {
            let mut n = 0;
            for j in 0..devices.len() {
                if devices[j].name != name {
                    continue;
                }
                devices[j].name = loop {
                    n += 1;
                    let numbered = format!("{name} ({n})");
                    if !devices.iter().any(|d| d.name == numbered) {
                        break numbered;
                    }
                };
            }
        }
    }
}

const RETIRE: Duration = Duration::from_millis(500);
const RETRY: Duration = Duration::from_millis(250);
const RETRIES: u32 = 8;
const NO_TURN: u64 = u64::MAX;

#[derive(Default)]
pub struct CpalOutput {
    device: Option<String>,
    plays_on: PlaysOn,
    events: Option<mpsc::Sender<Event>>,
    thread: Option<JoinHandle<()>>,
}

impl CpalOutput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on(device: Option<String>) -> Self {
        Self {
            device,
            plays_on: PlaysOn::Default,
            events: None,
            thread: None,
        }
    }
}

impl AudioOutput for CpalOutput {
    fn start(&mut self, source: AudioSource) -> Result<()> {
        let (events, received) = mpsc::channel::<Event>();
        let (ready, opened) = mpsc::channel::<Result<PlaysOn>>();
        let device = self.device.clone();
        let raise = events.clone();
        let thread = std::thread::Builder::new()
            .name("vidcut-audio-out".into())
            .spawn(move || {
                let turns = Arc::new(Turns::new());
                let open =
                    |turn, device: Option<&str>| open_stream(&source, &turns, turn, device, &raise);
                let (stream, plays_on, _) = match open(0, device.as_deref()) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready.send(Err(e));
                        return;
                    }
                };
                if ready.send(Ok(plays_on)).is_err() {
                    return;
                }
                follow(stream, &turns, &received, &source, |turn| {
                    open(turn, None).map(|(stream, _, name)| {
                        crate::debug!("audio", "the default output moved: playing on {name}");
                        stream
                    })
                });
            })
            .map_err(Error::Io)?;
        match opened.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(plays_on)) => {
                self.plays_on = plays_on;
                self.events = Some(events);
                self.thread = Some(thread);
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(_) => {
                let _ = events.send(Event::Stop);
                Err(Error::Invalid(
                    "the audio device didn't open within 5 s".into(),
                ))
            }
        }
    }

    fn plays_on(&self) -> PlaysOn {
        self.plays_on.clone()
    }
}

impl Drop for CpalOutput {
    fn drop(&mut self) {
        if let Some(events) = self.events.take() {
            let _ = events.send(Event::Stop);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

enum Event {
    Stop,
    Error {
        turn: u64,
        answer: Answer,
        why: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Nothing,
    Follow,
    Fail,
}

fn answer(kind: cpal::ErrorKind, on: &PlaysOn) -> Answer {
    use cpal::ErrorKind as K;
    match kind {
        K::Xrun | K::DeviceChanged | K::RealtimeDenied => Answer::Nothing,
        K::StreamInvalidated | K::DeviceNotAvailable if !matches!(on, PlaysOn::Chosen(_)) => {
            Answer::Follow
        }
        _ => Answer::Fail,
    }
}

fn follow<S>(
    first: S,
    turns: &Turns,
    events: &mpsc::Receiver<Event>,
    source: &AudioSource,
    mut open: impl FnMut(u64) -> Result<S>,
) {
    use mpsc::RecvTimeoutError as Gone;
    let (mut playing, mut turn, mut tries) = (first, 0, 0);
    let mut retiring: Option<(S, Instant)> = None;
    let mut retry: Option<Instant> = None;
    loop {
        let wake = retiring
            .as_ref()
            .map(|r| r.1)
            .into_iter()
            .chain(retry)
            .min();
        let event = match wake {
            Some(at) => events.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => events.recv().map_err(|_| Gone::Disconnected),
        };
        let now = Instant::now();
        match event {
            Ok(Event::Stop) | Err(Gone::Disconnected) => return,
            Err(Gone::Timeout) => {
                if retiring.as_ref().is_some_and(|r| r.1 <= now) {
                    retiring = None;
                    if let Some(gap) = turns.gap(turn) {
                        crate::debug!(
                            "audio",
                            "the new output's sound began {gap:+.0} ms after the old one's ended"
                        );
                    }
                }
                if !retry.is_some_and(|at| at <= now) {
                    continue;
                }
            }
            Ok(Event::Error { turn: of, .. }) if of != turn => continue,
            Ok(Event::Error {
                answer: Answer::Nothing,
                ..
            }) => continue,
            Ok(Event::Error {
                answer: Answer::Follow,
                why,
                ..
            }) => {
                if turns.took(turn) {
                    tries = 0;
                }
                if tries > RETRIES {
                    source.fail(why);
                    continue;
                }
            }
            Ok(Event::Error { why, .. }) => {
                source.fail(why);
                continue;
            }
        }
        retry = None;
        tries += 1;
        match open(turn + 1) {
            Ok(stream) => {
                turn += 1;
                turns.switch(turn);
                source.recover();
                let old = std::mem::replace(&mut playing, stream);
                retiring = Some((old, Instant::now() + RETIRE));
            }
            Err(_) if tries <= RETRIES => retry = Some(Instant::now() + RETRY),
            Err(e) => source.fail(e.to_string()),
        }
    }
}

struct Turns {
    base: Instant,
    active: AtomicU64,
    filling: AtomicBool,
    last: AtomicU64,
    until: AtomicI64,
    gap: AtomicI64,
}

impl Turns {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            active: AtomicU64::new(0),
            filling: AtomicBool::new(false),
            last: AtomicU64::new(NO_TURN),
            until: AtomicI64::new(0),
            gap: AtomicI64::new(0),
        }
    }

    fn fill(&self, turn: u64, source: &AudioSource, out: &mut [f32], heard_at: Instant) {
        if self.active.load(Ordering::Acquire) != turn || self.filling.swap(true, Ordering::Acquire)
        {
            out.fill(0.0);
            return;
        }
        if self.active.load(Ordering::Acquire) == turn {
            source.fill(out, heard_at);
            let from = heard_at.saturating_duration_since(self.base).as_nanos() as i64;
            let frames = (out.len() / usize::from(source.channels().max(1))) as i64;
            let lasts = frames * 1_000_000_000 / i64::from(source.rate().max(1));
            if self.last.swap(turn, Ordering::Relaxed) != turn {
                let until = self.until.load(Ordering::Relaxed);
                self.gap.store(from - until, Ordering::Relaxed);
            }
            self.until.store(from + lasts, Ordering::Relaxed);
        } else {
            out.fill(0.0);
        }
        self.filling.store(false, Ordering::Release);
    }

    fn switch(&self, turn: u64) {
        self.active.store(turn, Ordering::Release);
    }

    fn took(&self, turn: u64) -> bool {
        self.last.load(Ordering::Relaxed) == turn
    }

    fn gap(&self, turn: u64) -> Option<f64> {
        self.took(turn)
            .then(|| self.gap.load(Ordering::Relaxed) as f64 / 1e6)
    }
}

pub fn output_device() -> Result<String> {
    use cpal::traits::HostTrait;
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| Error::Invalid("no audio output device".into()))?;
    Ok(device.to_string())
}

fn output_by_id(host: &cpal::Host, id: &str) -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    host.output_devices()
        .ok()?
        .find(|d| d.id().is_ok_and(|d| d.to_string() == id))
}

fn open_stream(
    source: &AudioSource,
    turns: &Arc<Turns>,
    turn: u64,
    device: Option<&str>,
    raise: &mpsc::Sender<Event>,
) -> Result<(cpal::Stream, PlaysOn, String)> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let default = || {
        host.default_output_device()
            .ok_or_else(|| Error::Invalid("no audio output device".into()))
    };
    let (device, plays_on) = match device {
        None => (default()?, PlaysOn::Default),
        Some(id) => match output_by_id(&host, id) {
            Some(d) => (d, PlaysOn::Chosen(id.to_string())),
            None => (default()?, PlaysOn::Missing(id.to_string())),
        },
    };
    let config = cpal::StreamConfig {
        channels: source.channels(),
        sample_rate: source.rate(),
        buffer_size: cpal::BufferSize::Default,
    };
    let (filled, turns) = (source.clone(), turns.clone());
    let (raise, on) = (raise.clone(), plays_on.clone());
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |out, info| {
                let t = info.timestamp();
                let latency = t.playback.duration_since(t.callback);
                turns.fill(turn, &filled, out, Instant::now() + latency);
            },
            move |e| {
                let answer = answer(e.kind(), &on);
                if answer != Answer::Nothing {
                    let why = e.to_string();
                    let _ = raise.send(Event::Error { turn, answer, why });
                }
            },
            None,
        )
        .map_err(|e| {
            Error::Invalid(format!(
                "the audio device won't play {} Hz in {} channels: {e}",
                config.sample_rate, config.channels
            ))
        })?;
    stream
        .play()
        .map_err(|e| Error::Invalid(format!("the audio device won't start: {e}")))?;
    let name = device
        .description()
        .map_or_else(|_| "an unnamed device".into(), |d| d.name().to_string());
    Ok((stream, plays_on, name))
}

#[derive(Default)]
pub struct Recording {
    pub samples: Vec<f32>,
    pub heard: Vec<(usize, Instant)>,
}

impl Recording {
    pub fn heard_at(&self, i: usize, rate: u32) -> Option<Instant> {
        let at = self.heard.partition_point(|(first, _)| *first <= i);
        let (first, t) = self.heard.get(at.checked_sub(1)?)?;
        Some(*t + Duration::from_secs_f64((i - first) as f64 / f64::from(rate)))
    }
}

pub struct PacedOutput {
    period: Duration,
    latency: Duration,
    recording: Option<Arc<Mutex<Recording>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PacedOutput {
    pub fn new(period: Duration, latency: Duration) -> Self {
        Self {
            period,
            latency,
            recording: None,
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }

    pub fn recording(period: Duration, latency: Duration) -> (Self, Arc<Mutex<Recording>>) {
        let recording = Arc::new(Mutex::new(Recording::default()));
        let mut out = Self::new(period, latency);
        out.recording = Some(recording.clone());
        (out, recording)
    }
}

impl AudioOutput for PacedOutput {
    fn start(&mut self, source: AudioSource) -> Result<()> {
        let (period, latency) = (self.period, self.latency);
        let recording = self.recording.clone();
        let stop = self.stop.clone();
        let frames = ((period.as_secs_f64() * f64::from(source.rate())).round() as usize).max(1);
        let thread = std::thread::Builder::new()
            .name("vidcut-audio-out".into())
            .spawn(move || {
                let mut buf = vec![0.0f32; frames * usize::from(source.channels())];
                let mut due = Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    let now = Instant::now();
                    if due > now {
                        std::thread::sleep(due - now);
                    }
                    source.fill(&mut buf, due + latency);
                    if let Some(r) = &recording {
                        let mut r = r.lock().unwrap_or_else(|e| e.into_inner());
                        let first = r.samples.len() / usize::from(source.channels());
                        r.heard.push((first, due + latency));
                        r.samples.extend_from_slice(&buf);
                    }
                    due += period;
                }
            })
            .map_err(Error::Io)?;
        self.thread = Some(thread);
        Ok(())
    }
}

impl Drop for PacedOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[derive(Clone, Default)]
pub struct ManualOutput {
    source: Arc<Mutex<Option<AudioSource>>>,
}

impl ManualOutput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pull(&self, frames: usize, heard_at: Instant) -> Vec<f32> {
        let source = self.source.lock().unwrap_or_else(|e| e.into_inner());
        let Some(source) = source.as_ref() else {
            return Vec::new();
        };
        let mut out = vec![0.0; frames * usize::from(source.channels())];
        source.fill(&mut out, heard_at);
        out
    }

    pub fn queued(&self) -> u64 {
        let source = self.source.lock().unwrap_or_else(|e| e.into_inner());
        source.as_ref().map_or(0, |s| s.ring.queued())
    }
}

impl AudioOutput for ManualOutput {
    fn start(&mut self, source: AudioSource) -> Result<()> {
        *self.source.lock().unwrap_or_else(|e| e.into_inner()) = Some(source);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(samples: &[f32]) -> AudioSource {
        let ring = Arc::new(Ring::new(64, 1));
        ring.push(samples);
        let heard = Arc::new(Heard::new(Instant::now(), 1000));
        AudioSource::new(ring, heard, 1000, 1)
    }

    fn moved(turn: u64) -> Event {
        Event::Error {
            turn,
            answer: Answer::Follow,
            why: "the default moved".into(),
        }
    }

    fn failure(source: &AudioSource) -> Option<String> {
        source.failure.lock().unwrap().clone()
    }

    #[test]
    fn a_rerouted_stream_needs_nothing() {
        let on = PlaysOn::Default;
        assert_eq!(answer(cpal::ErrorKind::DeviceChanged, &on), Answer::Nothing);
    }

    #[test]
    fn a_stream_on_the_default_that_must_be_rebuilt_follows_it() {
        let on = PlaysOn::Default;
        assert_eq!(
            answer(cpal::ErrorKind::StreamInvalidated, &on),
            Answer::Follow
        );
    }

    #[test]
    fn a_chosen_device_gone_is_a_failure() {
        let on = PlaysOn::Chosen("wasapi:a".into());
        assert_eq!(
            answer(cpal::ErrorKind::DeviceNotAvailable, &on),
            Answer::Fail
        );
    }

    #[test]
    fn the_new_stream_takes_the_sound_where_the_old_one_left_it() {
        let source = source(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let turns = Turns::new();
        let (mut old, mut new) = ([0.0; 2], [0.0; 2]);
        turns.fill(0, &source, &mut old, Instant::now());
        turns.switch(1);
        turns.fill(0, &source, &mut old, Instant::now());
        turns.fill(1, &source, &mut new, Instant::now());
        assert_eq!((old, new), ([0.0, 0.0], [3.0, 4.0]));
    }

    #[test]
    fn the_gap_runs_from_the_old_streams_last_sound_to_the_new_ones_first() {
        let source = source(&[0.5; 20]);
        let turns = Turns::new();
        let at = turns.base + Duration::from_millis(100);
        turns.fill(0, &source, &mut [0.0; 10], at);
        turns.switch(1);
        turns.fill(1, &source, &mut [0.0; 10], at + Duration::from_millis(14));
        assert_eq!(turns.gap(1), Some(4.0));
    }

    #[test]
    fn a_default_output_that_moved_is_opened_again() {
        let source = source(&[1.0, 2.0]);
        let turns = Turns::new();
        turns.fill(0, &source, &mut [0.0; 2], Instant::now());
        let (events, received) = mpsc::channel();
        events.send(moved(0)).unwrap();
        events.send(Event::Stop).unwrap();
        let mut opened = Vec::new();
        follow((), &turns, &received, &source, |turn| {
            opened.push(turn);
            Ok(())
        });
        assert_eq!(opened, [1]);
    }

    #[test]
    fn a_stream_opened_again_ends_the_failure() {
        let source = source(&[1.0, 2.0]);
        source.fail("no audio output device".into());
        let turns = Turns::new();
        turns.fill(0, &source, &mut [0.0; 2], Instant::now());
        let (events, received) = mpsc::channel();
        events.send(moved(0)).unwrap();
        events.send(Event::Stop).unwrap();
        follow((), &turns, &received, &source, |_| Ok(()));
        assert_eq!(failure(&source), None);
    }

    #[test]
    fn an_error_from_the_stream_left_behind_is_no_failure() {
        let source = source(&[1.0, 2.0]);
        let turns = Turns::new();
        turns.fill(0, &source, &mut [0.0; 2], Instant::now());
        let (events, received) = mpsc::channel();
        events.send(moved(0)).unwrap();
        events
            .send(Event::Error {
                turn: 0,
                answer: Answer::Fail,
                why: "the old device went".into(),
            })
            .unwrap();
        events.send(Event::Stop).unwrap();
        follow((), &turns, &received, &source, |_| Ok(()));
        assert_eq!(failure(&source), None);
    }

    #[test]
    fn a_default_that_moved_before_any_sound_is_followed() {
        let source = source(&[]);
        let turns = Turns::new();
        let (events, received) = mpsc::channel();
        events.send(moved(0)).unwrap();
        events.send(Event::Stop).unwrap();
        let mut opened = Vec::new();
        follow((), &turns, &received, &source, |turn| {
            opened.push(turn);
            Ok(())
        });
        assert_eq!(opened, [1]);
    }

    #[test]
    fn a_default_that_keeps_moving_before_any_sound_gives_up() {
        let source = source(&[]);
        let turns = Turns::new();
        let (events, received) = mpsc::channel();
        events.send(moved(0)).unwrap();
        let mut opened = 0;
        follow((), &turns, &received, &source, |turn| {
            opened += 1;
            events.send(moved(turn)).unwrap();
            if opened == RETRIES + 1 {
                events.send(Event::Stop).unwrap();
            }
            Ok(())
        });
        assert_eq!(
            (opened, failure(&source)),
            (9, Some("the default moved".into()))
        );
    }

    #[test]
    fn one_entry_is_listed_for_each_output() {
        let device = |id: &str| OutputDevice {
            id: id.into(),
            name: id.into(),
            default: false,
        };
        let ids = [
            "alsa:null",
            "alsa:pipewire",
            "alsa:default",
            "alsa:hw:CARD=NVidia,DEV=3",
            "alsa:plughw:CARD=NVidia,DEV=3",
            "alsa:hdmi:CARD=NVidia,DEV=0",
            "alsa:dmix:CARD=NVidia,DEV=3",
            "alsa:hw:CARD=Mic,DEV=0",
            "alsa:sysdefault:CARD=Mic",
            "alsa:front:CARD=Mic,DEV=0",
            "alsa:surround51:CARD=Mic,DEV=0",
            "alsa:hw:CARD=Raw,DEV=0",
            "alsa:plughw:CARD=Raw,DEV=0",
            "wasapi:a",
        ];
        let mut devices: Vec<OutputDevice> = ids.iter().map(|id| device(id)).collect();
        let mut details = vec![None; ids.len()];
        one_for_each_output(&mut devices, &mut details);
        let kept: Vec<&str> = devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(
            kept,
            [
                "alsa:pipewire",
                "alsa:default",
                "alsa:hdmi:CARD=NVidia,DEV=0",
                "alsa:sysdefault:CARD=Mic",
                "alsa:hw:CARD=Raw,DEV=0",
                "wasapi:a",
            ]
        );
        assert_eq!(details.len(), 6);
    }

    #[test]
    fn a_card_listed_by_its_number_goes_where_its_named_twin_is() {
        let device = |id: &str, name: &str| OutputDevice {
            id: id.into(),
            name: name.into(),
            default: false,
        };
        let mut devices = vec![
            device("alsa:hw:CARD=NVidia,DEV=3", "HDA NVidia, MSI MP273QP"),
            device("alsa:hw:CARD=0,DEV=3", "HDA NVidia, MSI MP273QP"),
            device("alsa:hw:CARD=1,DEV=3", "HD-Audio Generic, HDMI 0"),
        ];
        let mut details = vec![
            Some("Direct hardware device without any conversions".into()),
            Some("Direct hardware device without any conversions".into()),
            Some("Direct hardware device without any conversions".into()),
        ];
        without_numbered_cards(&mut devices, &mut details);
        let ids: Vec<&str> = devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["alsa:hw:CARD=NVidia,DEV=3", "alsa:hw:CARD=1,DEV=3"]);
        assert_eq!(details.len(), 2);
    }

    #[test]
    fn repeated_device_names_are_told_apart() {
        let device = |id: &str, name: &str| OutputDevice {
            id: id.into(),
            name: name.into(),
            default: false,
        };
        let mut devices = [
            device("alsa:default", "Default ALSA Output"),
            device("alsa:sysdefault:CARD=PCH", "HDA Intel PCH, ALC897 Analog"),
            device("alsa:front:CARD=PCH,DEV=0", "HDA Intel PCH, ALC897 Analog"),
            device("wasapi:a", "Speakers"),
            device("wasapi:b", "Speakers"),
            device("wasapi:c", "Speakers (1)"),
        ];
        let details = [
            Some("Playback/recording through the PulseAudio sound server".into()),
            Some("Default Audio Device".into()),
            Some("Front output / input".into()),
            None,
            None,
            None,
        ];
        distinct_names(&mut devices, &details);
        let names: Vec<&str> = devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Default ALSA Output",
                "HDA Intel PCH, ALC897 Analog (Default Audio Device)",
                "HDA Intel PCH, ALC897 Analog (Front output / input)",
                "Speakers (2)",
                "Speakers (3)",
                "Speakers (1)",
            ]
        );
    }
}
