//! Where a player's sound goes: an audio device whose callback pulls the
//! mix, and with it the clock pictures follow.
//!
//! [`CpalOutput`] is a device of the system's (ALSA on Linux, which PipeWire
//! and PulseAudio serve; WASAPI on Windows; Core Audio on macOS): its
//! default, or one chosen by its id ([`output_devices`]). [`PacedOutput`] stands in for a device
//! without one: a thread that pulls in real time, optionally recording what
//! it pulled and when it would have been heard. [`ManualOutput`] pulls only
//! when a test says so, so a test sets the clock itself.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::clock::Heard;
use super::ring::Ring;
use crate::error::{Error, Result};

/// The player's end of an audio output: the device pulls from it.
#[derive(Clone)]
pub struct AudioSource {
    pub(crate) ring: Arc<Ring>,
    pub(crate) heard: Arc<Heard>,
    rate: u32,
    channels: u16,
    /// What went wrong with the device, for the player's status.
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

    /// Sample rate the device must run at.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Channels the device must have.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Fills `out` (interleaved) with what plays next, silence where nothing
    /// is queued (paused, or the mix is late: then the clock waits too).
    /// `heard_at`: when the first sample reaches the listener, the
    /// callback's time plus the output's latency. Doesn't block or
    /// allocate: for the device's own callback.
    pub fn fill(&self, out: &mut [f32], heard_at: Instant) {
        let (counter, n) = self.ring.pop(out);
        out[n * self.ring.channels()..].fill(0.0);
        self.heard.record(counter, n as u64, heard_at);
    }

    /// Reports a device failure (it went away, say).
    pub fn fail(&self, why: String) {
        *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
    }
}

/// Where the mix plays: a device that pulls from an [`AudioSource`], whose
/// pulling is the player's clock.
pub trait AudioOutput: Send {
    /// Starts pulling from `source`, at its rate and channel count, on the
    /// output's own thread, until the output is dropped.
    fn start(&mut self, source: AudioSource) -> Result<()>;

    /// The device it plays on, once started.
    fn plays_on(&self) -> PlaysOn {
        PlaysOn::Default
    }
}

/// The device an [`AudioOutput`] plays on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PlaysOn {
    /// The system's default, as none was chosen (or not a device at all:
    /// a stand-in).
    #[default]
    Default,
    /// The device chosen by its [`OutputDevice::id`].
    Chosen(String),
    /// The system's default, as the device chosen by this id isn't
    /// connected.
    Missing(String),
}

/// An output device of the system's.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OutputDevice {
    /// What [`CpalOutput::on`] opens it by: the same across runs and
    /// reboots where the system allows (`wasapi:…`, `alsa:…`).
    pub id: String,
    /// Its name, told apart from the others' ([`distinct_names`]).
    pub name: String,
    /// The system's default.
    pub default: bool,
}

/// The system's output devices, its default among them.
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
        // The description's line after the name.
        details.push(
            d.description()
                .ok()
                .and_then(|desc| desc.extended().nth(1).map(str::to_string)),
        );
    }
    distinct_names(&mut listed, &details);
    Ok(listed)
}

/// Names that repeat made distinct: ALSA's outputs of one card share the
/// card's name, told apart by their description's next line (`Front
/// output / input`, `Default Audio Device`); those still alike (a mixer's
/// two outputs named alike) are numbered, skipping numbers whose name
/// another device has.
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

/// An output device of the system's: its default, or one chosen by its
/// [`OutputDevice::id`], which plays on the default while it isn't there.
#[derive(Default)]
pub struct CpalOutput {
    /// The chosen device's id; `None`: the default.
    device: Option<String>,
    /// Where it plays, once started.
    plays_on: PlaysOn,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl CpalOutput {
    /// The system's default device.
    pub fn new() -> Self {
        Self::default()
    }

    /// The device of `id`, or the default for `None`.
    pub fn on(device: Option<String>) -> Self {
        Self {
            device,
            plays_on: PlaysOn::Default,
            stop: None,
            thread: None,
        }
    }
}

impl AudioOutput for CpalOutput {
    fn start(&mut self, source: AudioSource) -> Result<()> {
        let (stop, stopped) = mpsc::channel::<()>();
        let (ready, opened) = mpsc::channel::<Result<PlaysOn>>();
        let device = self.device.clone();
        // The stream lives on a thread of its own: it can't move between
        // threads on every platform.
        let thread = std::thread::Builder::new()
            .name("vidcut-audio-out".into())
            .spawn(move || {
                let (stream, plays_on) = match open_stream(source, device.as_deref()) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready.send(Err(e));
                        return;
                    }
                };
                let _ = ready.send(Ok(plays_on));
                let _ = stopped.recv();
                drop(stream);
            })
            .map_err(Error::Io)?;
        match opened.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(plays_on)) => {
                self.plays_on = plays_on;
                self.stop = Some(stop);
                self.thread = Some(thread);
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(Error::Invalid(
                "the audio device didn't open within 5 s".into(),
            )),
        }
    }

    fn plays_on(&self) -> PlaysOn {
        self.plays_on.clone()
    }
}

impl Drop for CpalOutput {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The system's default output device: its name, or why there is none.
pub fn output_device() -> Result<String> {
    use cpal::traits::HostTrait;
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| Error::Invalid("no audio output device".into()))?;
    Ok(device.to_string())
}

/// The output device whose id is `id`, as [`output_devices`] lists it.
/// Not cpal's own lookup by id: on ALSA it adds a device number to the id
/// it looks for (`sysdefault:CARD=PCH` becomes `…,DEV=0`), so the ids it
/// lists aren't found.
fn output_by_id(host: &cpal::Host, id: &str) -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    host.output_devices()
        .ok()?
        .find(|d| d.id().is_ok_and(|d| d.to_string() == id))
}

/// A stream from `source` on `device` (an [`OutputDevice::id`]), or on the
/// default device for `None` or a device that isn't there; and which it is.
fn open_stream(source: AudioSource, device: Option<&str>) -> Result<(cpal::Stream, PlaysOn)> {
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
    let failed = source.clone();
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |out, info| {
                let t = info.timestamp();
                let latency = t.playback.duration_since(t.callback);
                source.fill(out, Instant::now() + latency);
            },
            move |e| {
                if fails(e.kind()) {
                    failed.fail(e.to_string());
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
    Ok((stream, plays_on))
}

/// Whether a stream's error stops its sound. A glitch (an underrun) and a
/// route the stream followed (macOS's default output, moved to headphones)
/// don't: it plays on.
fn fails(kind: cpal::ErrorKind) -> bool {
    use cpal::ErrorKind as K;
    !matches!(kind, K::Xrun | K::DeviceChanged | K::RealtimeDenied)
}

/// What a [`PacedOutput`] pulled.
#[derive(Default)]
pub struct Recording {
    /// Every sample pulled, interleaved.
    pub samples: Vec<f32>,
    /// For each buffer pulled: the index in `samples` of its first sample
    /// frame, and when that would be heard.
    pub heard: Vec<(usize, Instant)>,
}

impl Recording {
    /// When sample frame `i` of `samples` would be heard.
    pub fn heard_at(&self, i: usize, rate: u32) -> Option<Instant> {
        let at = self.heard.partition_point(|(first, _)| *first <= i);
        let (first, t) = self.heard.get(at.checked_sub(1)?)?;
        Some(*t + Duration::from_secs_f64((i - first) as f64 / f64::from(rate)))
    }
}

/// A stand-in for a device: a thread that pulls `period` of sound every
/// `period`, each buffer heard `latency` after its pull is due.
pub struct PacedOutput {
    period: Duration,
    latency: Duration,
    recording: Option<Arc<Mutex<Recording>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PacedOutput {
    /// Drops what it pulls.
    pub fn new(period: Duration, latency: Duration) -> Self {
        Self {
            period,
            latency,
            recording: None,
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }

    /// Keeps what it pulls in the recording returned.
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
                // A device's clock is steady: each buffer is due a period
                // after the one before, however late the thread wakes.
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

/// An output that pulls only when told to: a test moves the clock itself,
/// buffer by buffer. Never told, it is no sound, for players that never
/// play (the two-up's).
#[derive(Clone, Default)]
pub struct ManualOutput {
    source: Arc<Mutex<Option<AudioSource>>>,
}

impl ManualOutput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pulls `frames` sample frames, the first heard at `heard_at`, as a
    /// device callback would; empty before the player starts it.
    pub fn pull(&self, frames: usize, heard_at: Instant) -> Vec<f32> {
        let source = self.source.lock().unwrap_or_else(|e| e.into_inner());
        let Some(source) = source.as_ref() else {
            return Vec::new();
        };
        let mut out = vec![0.0; frames * usize::from(source.channels())];
        source.fill(&mut out, heard_at);
        out
    }

    /// Sample frames queued for the device, ahead of what it pulled.
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

    #[test]
    fn a_followed_route_change_is_no_failure() {
        assert!(!fails(cpal::ErrorKind::DeviceChanged));
    }

    #[test]
    fn a_device_gone_is_a_failure() {
        assert!(fails(cpal::ErrorKind::DeviceNotAvailable));
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
            // Named as numbering would name the first.
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
