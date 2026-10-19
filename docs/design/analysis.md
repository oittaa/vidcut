# Analysis

The automatic tools (`edit.ripple_delete_silence`, `edit.detect_scene_cuts`) and the measurements (`analyze.silence`, `analyze.scenes`, `analyze.black`, `analyze.loudness`, `timeline.boring`) split in two: vidcut-av measures media, vidcut-core decides. Measuring decodes whole streams, so results are cached by the file's `Origin`, and a GUI measures on a job thread before it edits.

```mermaid
flowchart TD
  req["edit.ripple_delete_silence"] --> dispatch["Live::dispatch<br>vidcut-session"]
  dispatch -- "Live::measuring: media, streams" --> measure["Step::Measure<br>vidcut-measure, Work::Job"]
  measure --> levels["AvEnv::audio_levels<br>memo, disk, or measure"]
  levels -- "miss" --> av["vidcut_av::analysis::audio_levels"]
  av -- "stamped with the Origin" --> disk["analysis folder<br>in the cache dir"]
  levels -- "Msg::Measured" --> edit["Live::edit<br>Editor::apply"]
  edit -- "Env::audio_levels: memo hit" --> decide["analysis::silent_parts"]
  decide -- "frame ranges" --> ops["ops::remove_parts"]
```

## Measure, then decide

vidcut-core reaches media only through the `Env` trait. `AvEnv` implements it on vidcut-av; tests use `FakeEnv`. Decisions are plain functions of a measurement, a span and the settings.

| Request | Measurement (`Env`) | Decision (`analysis`) |
|---|---|---|
| `edit.ripple_delete_silence`, `analyze.silence` | `Env::audio_levels`: peaks of one audio stream | `analysis::silent_parts`; the edit then calls `ops::remove_parts` |
| `edit.detect_scene_cuts`, `analyze.scenes` | `Env::picture_stats`: the smaller of each frame's difference from the last and the change in that difference; the first frame scores 0 | `analysis::scene_cuts`; the edit then calls `ops::split_clip` |
| `analyze.black` | `Env::picture_stats`: each frame's black share | `analysis::black_parts` |
| `analyze.loudness` | `Env::loudness` of the flattened timeline | none |
| `timeline.boring` | none: clip lengths | `analysis::boring` |

## In a GUI

`Live::measuring` lists the media (and audio streams) of the clips the two edits name. A `vidcut-measure` thread at job priority, which yields to playback, measures them into `AvEnv`'s memo and disk cache and sends `Msg::Measured`. The session thread then runs the edit on the project as it is then; its measurements return at once. The `analyze` methods run whole on a job thread through `Live::project_task`. Headless, the edit measures inside its transaction.

**Why** measure first: the session thread answers every request and runs the players; decoding a whole file there would hold up both ([live](live.md)).

## Transcription

Create Subtitles from Audio (`subtitles.transcribe`) splits the same way. The editor flattens the range in one channel at 16 kHz (`RATE`), as an export would mix it, and asks `Env::transcribe` for what was said; vidcut-av's `transcribe::transcribe` hears it with transcribe.cpp, built in through the `transcribe-cpp` crate, and vidcut-core makes the cues.

```mermaid
flowchart TD
  req["subtitles.transcribe"] --> plan["the range's mix<br>16 kHz, one channel"]
  plan --> env["Env::transcribe<br>AvEnv, memo HEARD"]
  env -- "miss" --> files["transcribe::missing, download<br>models folder in the cache dir"]
  files --> mix["AudioMix: the samples"]
  mix --> vad["Silero VAD on the CPU<br>speech stretches"]
  vad --> pieces["transcript::pieces<br>up to transcribe::longest_piece"]
  pieces --> asr["the model on the GPU<br>Vulkan or Metal; the CPU when off"]
  asr --> said["Speech: what was said, with its times"]
  said --> cues["transcript::cues<br>sentences, lines, spread"]
  cues --> track["ops::subtitles::add_cue<br>the highest subtitle track"]
```

- **Model files** are pinned in `transcribe::File`: Hugging Face repository, revision, size and SHA-256. They live in the cache dir's `models` folder (`paths::models_dir`), which `vidcut reset` empties. `transcribe::missing` lists those not there; without `download` the request fails with DEPENDENCY, saying what it would fetch. `transcribe::download` reads each through FFmpeg's HTTPS (`avio_open2`), goes on from a `.part` file left before, and keeps the file only when its size and SHA-256 match. `model_file` and `vad_file` name files instead.
- **Pieces:** a model hears a piece at once. `transcript::pieces` puts the VAD's speech stretches together, with a little padding, up to the longest piece, and ends a piece at the longest pause in its second half; a stretch longer than a piece is cut where the sound is quietest. Each piece's times shift back by its start.
- **The GPU** is the one `gpus::in_use` names, found among transcribe.cpp's devices by its PCI slot or name; a failure to load there falls back to the CPU with a warning. `gpu.acceleration` off puts the model on the CPU. The longest piece goes first, and the language it finds is given to the rest.
- **Cues:** `transcript::cues` breaks the words into sentences and at pauses, splits a sentence too long for one cue into even parts, and wraps the lines. Words carry the model's times where it gives them; a model without times has its text spread over the speech of its piece by `transcript::spread` ([spec D61](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).

**Why** this piece length (`transcribe::longest_piece`): Qwen3-ASR's memory grows with the length it hears, so its pieces are sized from the GPU's memory (`Gpu::memory_mb`); a model without times needs short pieces to place its words; Whisper large-v3 loops in long ones ([gotchas](../gotchas.md#transcribecpp-and-its-models)). No CUDA: its backend and NVIDIA's runtime add about 930 MB, and it was slower than Vulkan for two of the three models.

## Caches

An `Origin` is a file's path, size and modification time (`fingerprint::origin`). Its name names the cache file; its text is stamped into it (`Origin::stamp`) and checked on read (`Origin::check`), so a changed file misses. The cache dir is `dirs::cache`.

| What | Where | Key |
|---|---|---|
| pool filmstrips, timeline tiles | `thumbs` (`thumbcache`) | origin, size, frames |
| waveforms | `waveforms` | origin, channels |
| audio levels, picture stats | `analysis` (`AvEnv`) | origin, stream or picture count |
| light, for HDR metadata | `analysis` (`light::cache_in`) | origin |
| proxies | `proxies.folder` (`Proxies::folder_in`): `proxies` beside the project unless set (in the cache dir without a project); an absolute folder is shared by every project | content (`paths::proxy_origin`), size, codec |
| GPU encoders that work | `encoders.json` (`export::detected`) | FFmpeg, system and GPUs |
| proxy codecs the GPU decoder plays | `plays.json` (`export::plays`) | the same, per GPU |
| clips for decoder tests | `decoder-tests` | none |
| compiled libplacebo shaders | `shaders` (`cuda::keep_shaders_in`) | libplacebo's |
| speech models | `models` (`paths::models_dir`) | file name, checked by size; SHA-256 when downloaded |

On disk only `thumbs` has a size limit (`thumbcache::evict`, least recently used first), set in the preferences. `Tiles::evict` also drops kept pictures past the tile cache's own budget. Levels, picture stats and light are memoized in the process too, and what was heard (`HEARD`) only there.

## Storyboards

`storyboard::render` flattens the timeline or a range and takes one picture per shot, from the middle of each source piece, or one every `every` frames. It decodes them with `FrameReader` and writes one PNG, each picture labeled with its timecode and clip, plus the track when the clip is not on V1.

## Code

- `crates/vidcut-core/src/analysis.rs`
- `crates/vidcut-core/src/editor.rs`
- `crates/vidcut-core/src/fingerprint.rs`
- `crates/vidcut-core/src/dirs.rs`
- `crates/vidcut-av/src/analysis.rs`
- `crates/vidcut-core/src/transcript.rs`
- `crates/vidcut-av/src/transcribe.rs`
- `crates/vidcut-av/src/light.rs`
- `crates/vidcut-av/src/waveform.rs`
- `crates/vidcut-av/src/thumbs.rs`
- `crates/vidcut/src/env.rs`
- `crates/vidcut/src/paths.rs`
- `crates/vidcut/src/thumbcache.rs`
- `crates/vidcut/src/live/mod.rs`
- `crates/vidcut/src/storyboard.rs`
