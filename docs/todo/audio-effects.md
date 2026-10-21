# Clip and track audio

**When:** next. Third in [the roadmap](../roadmap.md#next), beside [audio cleanup](audio-cleanup.md). Ducking, a voice EQ, a high-pass, a compressor, a limiter and a de-esser use the speech detection and the loudness export already have, so they can ship with no model.

Resolve does this in Fairlight (EQ, dynamics, a de-esser, and ducking on the Fairlight page). The Cut page has track volume and a clip's gain, which vidcut already has. vidcut has no mixer window; these are a request and a small window, and they are on the timeline a script already edits.

## Chain

One order, in `AudioMix`, so the viewer and the export hear the same samples. Earlier stages are [audio cleanup](audio-cleanup.md).

1. Repair: denoise, hum, dereverb, clicks.
2. High-pass, de-esser, EQ.
3. Compressor, then the limiter.
4. The clip gain that `clip.set` already sets (`gain_db`, −60 to 24).
5. Ducking.
6. The track fader and mute that `track.set` already sets.
7. On export only, the loudness measure and the one gain it already applies, holding the true peak at or under −1 dBTP (`audio.loudness`, `loudness_gain`).

Loudness sees the processed mix. There is no second limiter after it. A compressor the user set is the compressor that runs: nothing in the chain rescales its threshold, ratio, attack or release. The export's report names each stage that was on, the way it already names the loudness gain.

The processing is vidcut's own, in vidcut-av, over a fixed buffer a test can hash. FFmpeg's `highpass`, `equalizer`, `acompressor`, `alimiter` and `deesser` are LGPL filters in the `libavfilter` vidcut already links ([ffmpeg-filters](https://ffmpeg.org/ffmpeg-filters.html)); they are the reference for the shapes, not a subprocess and not a dependency of the hash.

## Controls

`audio.set` on clips or on tracks. Omitted fields stay as they were. A field at its default is omitted from the project file.

| Field | Default | Meaning |
|---|---|---|
| `highpass_hz` | off | a high-pass at this frequency; the voice preset's 80 Hz yields to this when both are set |
| `eq` | `flat` | `flat`, `voice`, or `bands` |
| `bands` | empty | each band: `hz`, `gain_db`, `q`, and `highpass`, `lowpass`, `peak`, `low_shelf` or `high_shelf` |
| `compressor` | off | `threshold_db`, `ratio`, `attack_ms`, `release_ms`, `makeup_db` |
| `limiter` | off | `ceiling_dbtp`, default −1 |
| `deesser` | off | `hz`, `amount` 0–1 |
| `duck` | off | `sources` (clips or tracks), `amount` 0–1, `attack_ms`, `release_ms` |

The `voice` preset, locked by a test, is a starting point for speech, not a copy of a Resolve preset: high-pass 80 Hz, a low shelf of −2 dB at 200 Hz, a peak of +3 dB at 3 kHz with Q 1, a high shelf of −1 dB at 10 kHz. Custom `bands` replace it.

The voice compressor, used when `compressor` is `voice`: threshold −18 dB, ratio 3, attack 10 ms, release 80 ms, and a makeup that restores the speech RMS. Any other object is those numbers as given. The limiter's ceiling of −1 dBTP matches the export's true-peak hold.

The de-esser is split-band on the band around `hz` (default 6 kHz, the range it cares about is about 5–8 kHz). `amount` 0 is off.

### Ducking

Ducking needs no new detector. Speech is the Silero activity vidcut already caches (`transcribe::voice_activity`, `analysis::non_speech`, [speech](../design/analysis.md#speech)). A music or bed track names the speech tracks in `sources`. While those sources are speech, its gain follows `amount` (1 is fully down, 0 is unchanged), with the attack and release. The envelope is sample-exact in `AudioMix`. A source that has not been measured is measured first, on a job, the way ripple-delete-silence measures. Ducking a track by its own speech is an error.

## Requests

Clip gain stays `vidcut clip set c3 --gain-db -3`. The track fader stays `vidcut track set A2 --volume-db -6`. `audio.set` does not take those.

```bash
vidcut audio set c3 --eq voice --compressor voice --deesser 0.4
vidcut audio set --tracks A2 --highpass-hz 100 --duck '{"sources":["A1"],"amount":0.7}'
vidcut audio get c3 --json
vidcut audio set c3 --clear
```

```json
{"method":"audio.set","params":{"clips":"c3","eq":"voice","compressor":"voice","limiter":{"ceiling_dbtp":-1}}}
```

MCP: `audio_set`, `audio_get`. The window is one Audio window for the selected clip or track: the preset, the high-pass, and Advanced for the bands, the compressor, the limiter, the de-esser and ducking. Done is one `audio.set`.

## Interchange

The published Fairlight effects vidcut already keeps are the clip volume and fades, and clip pan. No EQ, compressor or de-esser parameter ids are published, and none are invented. `audio.set`'s fields go in the clip's or the track's `vidcut` metadata. An OTIO export warns, naming them, that Resolve will not hear those stages. Volume, fades and pan already round-trip. Unknown Fairlight parameters in `Kept` stay.

## Tests

Each stage is one test with the samples written out: the voice preset's gain at 3 kHz; the high-pass attenuating a bin below its cutoff; the compressor's gain on a constant above its threshold; the limiter holding a peak at the ceiling; the de-esser attenuating a 6 kHz tone and leaving a 200 Hz tone; ducking a bed by a speech span the test gives as probabilities, so it does not run the VAD. A project omits a default `audio` object.

## Open questions

- Whether a track duck and a clip duck on the same sound multiply or the track wins. The first version multiplies, and a test locks that.
- The voice preset's four bands are a proposal. Change them in this file before anyone has a project that stored `eq: voice`.
