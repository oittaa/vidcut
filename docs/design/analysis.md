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
| `edit.ripple_delete_silence`, `analyze.silence` | `Env::audio_levels`: peaks of one audio stream; with `detect: speech`, `Env::voice_activity`: its speech probabilities | `analysis::silent_parts`, or `analysis::non_speech_parts`; the edit then calls `ops::remove_parts` |
| `edit.detect_scene_cuts`, `analyze.scenes` | `Env::picture_stats`: the smaller of each frame's difference from the last and the change in that difference; the first frame scores 0 | `analysis::scene_cuts`; the edit then calls `ops::split_clip` |
| `analyze.black` | `Env::picture_stats`: each frame's black share | `analysis::black_parts` |
| `analyze.loudness` | `Env::loudness` of the flattened timeline | none |
| `timeline.boring` | none: clip lengths | `analysis::boring` |

Given `parts`, `edit.ripple_delete_silence` measures nothing: it checks that each part lies inside its clip and apart from the clip's others, and cuts them.

## Speech

Ripple Delete Silence with `detect: speech` cuts what isn't speech. vidcut-av's `transcribe::voice_activity` mixes one audio stream in one channel at 16 kHz through `AudioMix`, as transcription listens (`listen`), and feeds it to Silero VAD on the CPU in pieces. `VoiceActivity` keeps its speech probability for each of the VAD's frames (32 ms). `analysis::non_speech` finds the speech in the clip's part of them by Silero's own rule, which transcribe.cpp ports: speech starts at a frame at or above the threshold, and ends at a frame more than 0.15 below it when no frame reaches the threshold again within Minimum to Trip; speech no longer than Shortest Speech doesn't count. What lies between is cut, less Pre Head and Post Tail, as for silence (`analysis::silent_parts`).

**Why** probabilities, not transcribe.cpp's segments: those depend on the threshold and the lengths, so each change in the dialog would run the VAD over the whole file again; the probabilities are measured once, cached like the levels, and decided again at once.

## In a GUI

`Live::measuring` lists the media (and audio streams) of the clips the two edits name. A `vidcut-measure` thread at job priority, which yields to playback, measures them into `AvEnv`'s memo and disk cache and sends `Msg::Measured`. The session thread then runs the edit on the project as it is then; its measurements return at once. The `analyze` methods run whole on a job thread through `Live::project_task`. Headless, the edit measures inside its transaction.

**Why** measure first: the session thread answers every request and runs the players; decoding a whole file there would hold up both ([live](live.md)).

## Transcription

Create Subtitles from Audio (`subtitles.transcribe`) splits the same way. With `hear: mix`, the editor flattens the range of the tracks heard (`heard_tracks`: the request's `tracks`, or those playback hears) in one channel at 16 kHz (`RATE`), as an export would mix it, and asks `Env::transcribe` for what was said; vidcut-av's `transcribe::transcribe` hears it with transcribe.cpp's prebuilt library through the `transcribe_cpp` crate, and vidcut-core makes the cues. With `hear: media`, the default for a model that times words (`hears_media`), `clip_speech` reads each heard clip's file's transcript instead, as `media.transcribe` keeps it, transcribing and keeping one that is missing or that another model or language made (`Asked::made`). It places the words through each clip's sound, J and L cuts included (`analysis::words_between`), keeps the first track's word where two tracks have the same one at once (`first_heard`), and hands the cues one `Speech` in the range's times. Re-time's words come the same way.

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

- **Model files** are pinned in `transcribe::File`: Hugging Face repository, revision, size, SHA-256 and the languages the model's metadata lists. They live in the cache dir's `models` folder (`paths::models_dir`), which `vidcut reset` empties. `transcribe::needed` says what of transcribe.cpp's library, Silero VAD and the speech model isn't there; without `download` the request fails with DEPENDENCY, saying what it would fetch. `transcribe::fetch` reads each through FFmpeg's HTTPS (`avio_open2`), goes on from a `.part` file left before, and keeps the file only when its size and SHA-256 match. `model_file` and `vad_file` name files instead.
- **The GUI's dialog** (`Workspace::speech_window`) asks `transcribe::needed` and `transcribe::short_of_memory` before it sends the request, so it can ask before a download and warn before the run; the run is a job like the CLI's.
- **transcribe.cpp's library is downloaded, not built** (`transcribe::RUNTIME`): the prebuilt bundle transcribe.cpp publishes for this system (Windows and Linux on Vulkan, macOS on Metal), pinned by size and SHA-256 and unpacked into the `models` folder (`transcribe::unpack`). crates/transcribe-cpp-sys stands in for the crate of that name (patched in by the workspace): the bindings that crate ships, rewritten by scripts/transcribe-sys.py so that each function calls the library `transcribe::start` loads (`transcribe_cpp_sys::load`), which then starts the backends beside it. Silero VAD is a pinned file like the models (`transcribe::SILERO_VAD`). `transcribe::get_ready` gets them before anything is heard, for `subtitles.transcribe`, `subtitles.retime`, and `edit.ripple_delete_silence` and `analyze.silence` with `detect: speech`, each of which takes `download`.
- **Finding the speech:** Silero VAD's stretches, or with `find: sound` the stretches louder than `sound_db` that `transcript::sounding` finds in the levels of 32 ms frames, parted by half a second of quiet. With the voice detection, `missed_singing` warns when it keeps under a quarter of the sound of a range that is mostly loud. A kept transcript records how it was found and at what level (`Speech::found`, `Asked::level`), and `Asked::made` reuses it only for the same `find` and level; the memo of the mix heard (`heard_key`) keys on them too.
- **Pieces:** a model hears a piece at once. `transcript::pieces` puts the VAD's speech stretches together, with a little padding, up to the longest piece, and ends a piece at the longest pause in its second half; a stretch longer than a piece is cut where the sound is quietest. Each piece's times shift back by its start.
- **The GPU** is the one `gpus::in_use` names, found among transcribe.cpp's devices by its PCI slot or name; a failure to load there falls back to the CPU with a warning. `gpu.acceleration` off puts the model on the CPU. The longest piece goes first, and the language it finds is given to the rest.
- **Cues:** `transcript::cues` breaks the words into sentences and at pauses, splits a sentence too long for one cue into even parts, and wraps the lines. Words carry the model's times where it gives them; a model without times has its text spread over the speech of its piece by `transcript::spread` ([spec D62](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Word times on cues:** `transcript::cues` keeps each word's start in its cue (`Made`). Synchronize Word Timings (`subtitles.time_words`) hears the cues' span as Create Subtitles from Audio does, and `retime::word_starts` matches the cues' words to the words heard in order across them, by the longest common runs as Re-time's words do; `subtitles::fill_times` spreads the words left between their matched neighbors by characters. New text with another number of words in `subtitles.set` is read again from the words kept for the clips' files alone (`clip_speech` told not to transcribe), else spread ([spec D70](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Transcripts of media:** `media.transcribe` hears one pool item's sound stream on its own: the Source Tape's plan of it (`render::tape`) at 16 kHz, mono, with the stream asked for. `Env::transcribe_media` keeps the `Speech` as JSON with the file's other measurements (`words0`, `words1`, … per stream), stamped with its origin, so a changed file has none; `Env::transcript` reads it back, and it is used again while the model asked made it, in the language asked (`Asked::made`; `SpeechModel::title` names the model in both). Only a transcript with word times is kept (`transcript::spoken`), and a model known not to time words (`SpeechModel::times_words`) is refused before anything is heard. The core's `transcribed` set, the pool's mark, comes from which of those files exist (`env::transcribed`).
- **Words:** `analyze.words` with `media` (the first stream transcribed, unless `stream`) or `clips` (each clip's own stream) reads those transcripts and hears nothing: a word lands on a clip through its source range and speed (`analysis::words_in`), when its middle is inside the clip. Without them it hears the timeline's mix the same way as `subtitles.transcribe` and returns each word `transcript::spoken` kept, at its frames (`transcript::word_frames`). It refuses a model whose times are phrases or none: the spread estimates are not word times. `edit.ripple_delete_words` takes those words' spans out through `ops::cut_ranges`, which closes the gap whether or not ripple is on ([spec D63](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)). With `quiet`, each edge first moves to the quietest frame boundary nearby (`analysis::quietest`), from the levels of the clips heard there, bounded by the words the clips' transcripts place: no further than a third into any word wholly inside the cut, and outward only to the next word; the core measures levels it doesn't keep yet on a job first (`Live::measuring`). Words are marked as fillers (`transcript::is_filler`) in the transcript's language, else the one `transcript::language_of` finds from the commonest words, by the project's list for it (`Settings::fillers`), else the preferences' (`Env::fillers`), else the built-in one (`transcript::built_in_fillers`).

**Why** this piece length (`transcribe::longest_piece`): Qwen3-ASR's memory grows with the length it hears, so its pieces are sized from the GPU's memory (`Gpu::memory_mb`); a model without times needs short pieces to place its words; Whisper large-v3 loops in long ones ([gotchas](../gotchas.md#transcribecpp-and-its-models)). Parakeet Ultra is the default (`SpeechModel`): it gives real word times now, where Qwen3-ASR's are estimates without transcribe.cpp's aligner. No CUDA: its backend and NVIDIA's runtime add about 930 MB, and it was slower than Vulkan for two of the three models.

## Re-timing subtitles

Re-time to Speech (`subtitles.retime`) moves a subtitle track's cues to the timeline's speech. It measures with the two above and decides in vidcut-core's `retime`.

```mermaid
flowchart TD
  req["subtitles.retime"] --> heard["heard_speech: each clip heard<br>Env::voice_activity, analysis::non_speech_parts"]
  heard --> line["retime::line_up<br>the speech against where the cues show"]
  line -- "words" --> asr["Env::transcribe<br>Parakeet Ultra, the whole timeline"]
  asr --> anchors["retime::anchors<br>runs of 3 words or more"]
  anchors --> levels["retime::levels<br>offset, speed, sections, words"]
  line -- "no words, clear" --> plan["retime::Plan"]
  levels -- "the first that fits" --> plan
  plan --> set["ops::subtitles::set_cue"]
```

- **Speech** is where the cached voice activity of each clip on the tracks heard (`heard_tracks`) is speech by Silero's rule with its defaults (`SPEECH` 0.5, `SHORTEST_SPEECH` 0.25 s, `SHORTEST_PAUSE` 0.1 s), on the timeline through the clip (`Span`).
- **Lining up** (`retime::line_up`) scores each offset, every 10 ms up to `max_offset`, by the cross-correlation of the speech, less its mean, with where the cues show. It is summed exactly, as the trapezoid each speech stretch and cue make against the offset (`Ramps`), not sampled. Speeds from 0.95 to 1.05 are tried in steps that move the last cue 0.25 s, then a tenth of that around the best. A speed within 0.25 s of 1 or of a frame-rate ratio over the track snaps to it (`retime::snap`), and stays only when it scores above speed 1. The best offset is clear when it scores `CLEAR` (1.25) times the highest other peak more than 2 s from it.
- **Words** (`retime::anchors`): the cues' text and the words heard, as lower-case letters and digits, are matched in order by their longest common runs, as Python's difflib does without its junk heuristic. A cue whose first word is in a run of `RUN` (3) words or more is anchored at that word's start, and at its last word's end when that word matched in the same run, so that only the cue's own words lie between; otherwise the cue keeps its length.
- **Levels** (`retime::levels`) fit the anchors' offsets (the word's start less the cue's): their median; a Theil–Sen line, its speed snapped as above; sections, each starting where 5 anchors in a row sit more than 0.5 s from the last section's offset and within 0.5 s of their own median; and last, each anchored cue at its words, the others as in their section. The first that puts 90% of the anchors within `FIT` (0.5 s) wins. Under `MIN_ANCHORS` (10) the words decide nothing, and a clear offset from the speech stands.
- **In a GUI** it runs as a job (`Step::Encode`) on a copy, so the VAD and the transcription run off the core's thread; the edit then finds both cached.

**Why** the speech first, not the words: where speech is doesn't depend on its language, so a translated file lines up as well as the original. **Why** 1.25: over the NASA press conference and its captions (nasa-roman-presser in the media manifest), lined up in 5-minute windows, the one window whose offset was wrong scored 1.09 times its next peak, and the last, 76 s long, 1.07 though right; the other ten scored 1.38 or more. The whole file scored 2.03 at +2.94 s, where Parakeet Ultra's words put it at +3.02 s.

## Caches

An `Origin` is a file's path, size and modification time (`fingerprint::origin`). Its name names the cache file; its text is stamped into it (`Origin::stamp`) and checked on read (`Origin::check`), so a changed file misses. The cache dir is `dirs::cache`.

| What | Where | Key |
|---|---|---|
| pool filmstrips, timeline tiles | `thumbs` (`thumbcache`) | origin, size, frames, input color space |
| waveforms | `waveforms` | origin, channels |
| audio levels, voice activity, picture stats | `analysis` (`AvEnv`) | origin, stream or picture count |
| transcripts (`media.transcribe`) | `analysis` (`AvEnv`) | origin, stream |
| light, for HDR metadata | `analysis` (`light::cache_in`) | origin, input color space |
| proxies | `proxies.folder` (`Proxies::folder_in`): `proxies` beside the project unless set (in the cache dir without a project); an absolute folder is shared by every project | content (`paths::proxy_origin`), size, codec, input color space |
| GPU encoders that work | `encoders.json` (`export::detected`) | FFmpeg, system and GPUs |
| proxy codecs the GPU decoder plays | `plays.json` (`export::plays`) | the same, per GPU |
| clips for decoder tests | `decoder-tests` | none |
| compiled libplacebo shaders | `shaders` (`cuda::keep_shaders_in`) | libplacebo's |
| speech models | `models` (`paths::models_dir`) | file name, checked by size; SHA-256 when downloaded |

On disk only `thumbs` has a size limit (`thumbcache::evict`, least recently used first), set in the preferences. `Tiles::evict` also drops kept pictures past the tile cache's own budget. Levels, voice activity, picture stats and light are memoized in the process too, and what was heard (`HEARD`) only there.

## Storyboards

`storyboard::render` flattens the timeline or a range and takes one picture per shot, from the middle of each source piece, or one every `every` frames. It decodes them with `FrameReader` and writes one PNG, each picture labeled with its timecode and clip, plus the track when the clip is not on V1.

## Code

- `crates/vidcut-core/src/analysis.rs`
- `crates/vidcut-core/src/editor.rs`
- `crates/vidcut-core/src/fingerprint.rs`
- `crates/vidcut-core/src/dirs.rs`
- `crates/vidcut-av/src/analysis.rs`
- `crates/vidcut-core/src/transcript.rs`
- `crates/vidcut-core/src/retime.rs`
- `crates/vidcut-av/src/transcribe.rs`
- `crates/vidcut-av/src/light.rs`
- `crates/vidcut-av/src/waveform.rs`
- `crates/vidcut-av/src/thumbs.rs`
- `crates/vidcut/src/env.rs`
- `crates/vidcut/src/paths.rs`
- `crates/vidcut/src/thumbcache.rs`
- `crates/vidcut/src/live/mod.rs`
- `crates/vidcut/src/storyboard.rs`
