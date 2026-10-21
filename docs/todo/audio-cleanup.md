# Audio cleanup

**When:** next. Second in [the roadmap](../roadmap.md#next), with [clip and track audio](audio-effects.md). A Meet recording (use case 2) is still noisy after the silences are cut. The denoiser runs on a laptop CPU.

Local models only. Nothing is uploaded. A model is fetched the way a speech model is: pinned by repository, revision, size and SHA-256, kept in the cache dir's `models` folder, refused with `DEPENDENCY` until `download` is set, and the GUI asks before the fetch ([analysis](../design/analysis.md#transcription)). Weights are MIT or Apache-2.0. There is no PyTorch runtime and no Python subprocess.

## What runs

`audio.clean` on clips or on tracks. Each flag is off unless the request sets it, so a clip's sound is unchanged until someone asks.

| Flag | Does |
|---|---|
| `noise` | speech enhancement |
| `model` | `deepfilter` (the default when `noise` is on) or `rnnoise` |
| `strength` | 0–1, how hard the model is driven; 1 is the model's own mix |
| `hum` | `auto`, `50`, `60` or `off`; a notch comb at that mains frequency and its harmonics |
| `reverb` | a dereverb |
| `clicks` | mouth clicks |
| `separate` | vocals apart from the rest, an offline job |
| `download` | may fetch a missing model |

`hum` `auto` measures 50 Hz against 60 Hz on the clip and uses the stronger. `strength` scales a model's wet mix; it does not retune a network.

Repair is the first stage of the chain in [clip and track audio](audio-effects.md#chain). The enhanced samples are cached by the file's origin, the model, the revision and the settings, the way a transcript is. Preview and export both hear `AudioMix`, so they match.

### Noise

DeepFilterNet3 is the default. The repository is dual-licensed MIT or Apache-2.0 ([Rikorose/DeepFilterNet](https://github.com/Rikorose/DeepFilterNet), `libDF/Cargo.toml` license `MIT/Apache-2.0`). The paper reports a real-time factor of 0.19 on one thread of a notebook CPU, and publishes the weights under an open-source license ([arXiv:2305.08227](https://arxiv.org/abs/2305.08227)). It is full-band, 48 kHz, frames of a few tens of milliseconds, and the inference runs in Rust on `tract`.

Pin a git revision of `deep_filter` 0.5 (the tree's `libDF/Cargo.toml` is `0.5.7-pre`, features `tract` and `default-model`). The crate published on crates.io is the old 0.2 line and is not the one to depend on. The ONNX weights are pinned like a speech model, with their SHA-256.

With `gpu.acceleration` on, a GPU provider runs it when that provider's samples match the CPU's on a short fixture; the answer names the provider. When they differ, or when acceleration is off, it runs on the CPU, and the answer says why. A laptop with acceleration off is the real-time path the paper measured. Playback may enhance a short look-ahead and keep the rest cached; it does not drop to a lighter model on its own.

RNNoise is the light fallback ([xiph/rnnoise](https://github.com/xiph/rnnoise), BSD-3-Clause). FFmpeg's `arnndn` is the LGPL `libavfilter` filter vidcut already links: an external `.rnnn` model and a mix from −1 to 1 ([ffmpeg-filters, arnndn](https://ffmpeg.org/ffmpeg-filters.html#arnndn)). `model: rnnoise`, or DeepFilterNet when its real-time factor on this machine is above 1, uses it. The model file is pinned and hashed. No `ffmpeg` subprocess.

### Hum, reverb, clicks

Hum is a comb of notches, no model. The frequencies and the gains are constants a unit test locks.

DeepFilterNet is a denoiser. A mild room uses a linear prediction dereverb (weighted prediction error) with no download. A learned dereverb is added only as an ONNX or `tract` model under MIT or Apache-2.0, pinned and hashed the same way. ClearerVoice's MossFormer2 enhancement is Apache-2.0 and does remove reverberation, and it is a Python runtime ([modelscope/ClearerVoice-Studio](https://github.com/modelscope/ClearerVoice-Studio)); it is not a dependency. The first version ships the linear dereverb and leaves a learned one out until a small model qualifies.

Mouth clicks have no small model with a license vidcut can ship. The detector is classical and off unless `clicks` is set: a short impulsive peak in the band from about 2 kHz to 8 kHz, inside a pause of the speech activity vidcut already measures, ducked with a gain dip of a few milliseconds. The existing de-click at a cut is a different fade (a fixed 3 ms at every audio cut). A dip in the wrong place takes out a consonant, so the answer reports how many dips it made, and `audio.clean` with `clicks` and a dry run lists them without writing. It will not match a proprietary restoration suite.

### Vocals apart from the rest

Demucs v4 (`htdemucs`) separates vocals, drums, bass and other. The code is MIT ([facebookresearch/demucs](https://github.com/facebookresearch/demucs)). Its README says the repository is unmaintained. It is a heavy offline model, not a playback effect, and vidcut does not shell out to Python.

`separate` is a job, cached like an analysis. With `gpu.acceleration` on it uses the GPU when the GPU's stems match a CPU fixture; otherwise the CPU, and the answer gives the rate. The weights have to be MIT or Apache-2.0 and are pinned by SHA-256. Playback never waits on it. The first version may ship `noise` without `separate` if no small vocal model of that license is pinned; the request still exists and answers that the model is not available.

## Requests

```bash
vidcut audio clean c3 --noise --download
vidcut audio clean c3 --noise --model rnnoise --hum auto --clicks
vidcut audio clean --tracks A2 --noise --strength 0.6
```

```json
{"method":"audio.clean","params":{"clips":"c3","noise":true,"hum":"auto","download":true}}
```

MCP: `audio_clean`. `audio.clean` with `clear` removes the cleanup on those clips or tracks. The settings are stored on the clip or the track in the project, omitted when off, and the samples are a cache, not part of the file.

## Interchange

Resolve's OTIO carries Fairlight volume and pan. It has no published parameter ids for a denoiser, a notch or a separator, and vidcut does not invent them. The settings go in the clip's `vidcut` metadata ([OpenTimelineIO](../design/model.md#opentimelineio)). An OTIO export warns, naming the clips, that Resolve will not hear the cleanup. Unknown Fairlight parameters already in `Kept` stay. A media export has the cleanup in the mix, because the export matches the viewer.

## Tests

A missing model with no `download` is `DEPENDENCY`, the message naming the file and the hash. Hum: one sine at 60 Hz and one at 1 kHz, the 60 Hz bin down by a gain the test writes out. Clicks: one impulse between two voiced frames is dipped, a voiced impulse is not. The model itself is a hand check on a short fixture in `$VIDCUT_TEST_DATA`, hashed, not a unit test that reimplements the network.

## Open questions

- Which DeepFilterNet3 ONNX file is the pinned default (`DeepFilterNet3_onnx` or the low-latency `ll` file), decided by a real-time factor measured on a laptop CPU, both hashes recorded.
- A learned dereverb small enough to run in `tract`, if one with a usable license appears. Until then the linear dereverb is the implementation.
- A vocal separator smaller than Demucs, MIT or Apache-2.0, before `separate` does any work.
