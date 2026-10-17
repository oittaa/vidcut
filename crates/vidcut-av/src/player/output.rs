use std::sync::atomic::{AtomicBool, Ordering};
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

#[derive(Default)]
pub struct CpalOutput {
    device: Option<String>,
    plays_on: PlaysOn,
    stop: Option<mpsc::Sender<()>>,
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

fn fails(kind: cpal::ErrorKind) -> bool {
    use cpal::ErrorKind as K;
    !matches!(kind, K::Xrun | K::DeviceChanged | K::RealtimeDenied)
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

    #[test]
    fn a_followed_route_change_is_no_failure() {
        assert!(!fails(cpal::ErrorKind::DeviceChanged));
    }

    #[test]
    fn a_device_gone_is_a_failure() {
        assert!(fails(cpal::ErrorKind::DeviceNotAvailable));
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
