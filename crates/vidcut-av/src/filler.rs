//! Filler media for timeline playback: black video and silent audio that fill
//! gaps in mpv's EDL (docs/decisions/preview.md), cached per format.

use std::path::Path;

use vidcut_core::preview::Fillers;
use vidcut_core::time::Rational;

use crate::error::Result;
use crate::fixture::{self, AudioCodec, Container, FixtureSpec, Picture, Sound, VideoCodec};

/// Seconds of media in each filler file (longer gaps chain several entries).
const SECONDS: u32 = 10;

/// Black video at `width`×`height` and `fps`, and 48 kHz stereo silence,
/// generated into `dir` if not there yet.
pub fn ensure(dir: &Path, width: u32, height: u32, fps: Rational) -> Result<Fillers> {
    let frames = (f64::from(SECONDS) * fps.as_f64()).round() as u32;
    let rate = (fps.num() as u32, fps.den() as u32);
    let black = FixtureSpec::new(
        VideoCodec::H264,
        Container::Mkv,
        width,
        height,
        rate,
        frames,
    )
    .picture(Picture::Black)
    .no_audio()
    .bframes(0)
    .gop(frames.max(1));
    let silence = FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, rate, frames)
        .audio_only()
        .sound(Sound::Silence)
        .with_audio(AudioCodec::Flac, 48_000, 2);
    let secs = f64::from(frames) / fps.as_f64();
    Ok(Fillers {
        black: fixture::ensure_in(dir, &black)?,
        black_secs: secs,
        silence: fixture::ensure_in(dir, &silence)?,
        silence_secs: secs,
    })
}
