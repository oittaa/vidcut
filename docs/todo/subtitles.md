# Subtitles

**When:** awaits the user's decision on this proposal: which phases, if any. Until then subtitles stay out of scope ([spec §1](../cut-page-spec.md#1-scope)).

A proposal, not a plan of record. Subtitles serve both [use cases](../roadmap.md#use-cases): an agent cuts by what is said (1), and a meeting recording gets captions (2).

## What Resolve does

The Cut page has the Edit page's subtitles (p. 702): Timeline Actions > Add Subtitle Track and Create Subtitles from Audio (Studio), Timeline Options > Minimize Subtitle Track (pp. 645–646). The rest is in the Subtitles and Closed Captioning chapter (pp. 1227–1248):

- **Tracks:** ST1, ST2, … above the video tracks, one per language, named for export (ISO 639 codes). Lock, enable and a destination control; only one is shown at a time (p. 1228). Up to three regions per track for cues on screen together (p. 1243). Warnings for characters per line, minimum duration and characters per second (pp. 1228–1229).
- **Cues** are subtitle clips, moved, trimmed, rolled, slipped and rippled like other clips (p. 1237). Added at the playhead, typed in the Inspector, found and replaced (pp. 1232–1236); fades (p. 1240); linked to a clip to move with it (p. 1242).
- **Style:** per track (font, size, color, position, shadow, stroke, background), per cue and per word; presets; Fusion templates and animated templates (pp. 1237–1242).
- **Import:** `.srt`, `.srtx`, `.vtt`, `.xml`, `.ttml` come into the Media Pool as a subtitle clip, which breaks into one clip per cue when dropped on a track, or goes in at its timecode (p. 1230).
- **Export:** File > Export Subtitle or the track header: SRT, WebVTT (p. 1247). The Deliver page: a separate file per track (IMSC1, DFXP, SRT, WebVTT), burned into the picture, or embedded (pp. 1247–1248).
- **Create Subtitles from Audio** (Studio): language, caption preset, characters per line, one or two lines, gap; writes the top subtitle track (pp. 1234–1235). Clip transcription with word timings, and editing a timeline by its text, are Studio's too, in the Transcription window (pp. 714, 995–1009).

Checked with Resolve Studio 21.1.1's scripting API in a throwaway project:

- An SRT appended to a subtitle track becomes one clip per cue, keeping the file's times from where it lands, rounded to frames (0.5 s at 25 fps to frame 13). `<i>` is dropped from the text; a line break becomes U+2028.
- `CreateSubtitlesFromAudio` captioned 22 s of synthetic speech in 9 s on this PC (NVIDIA GPU): the right words, "ten percent" as "10%", cues cut at 42 characters across sentences. `TranscribeAudio` and `GetTranscription` give each word's start and end in frame timecode, silences as `(...)`.
- Rendering with `SubtitleFormat` `SeparateFile` wrote IMSC1 TTML in timeline timecode (the API can't choose the format); `BurnIn` drew white bold text in the lower third; `EmbeddedCaptions` gave Matroska a SubRip stream timed from the timeline's start, and failed for H.264 in MP4 and MOV ("Cannot find appropriate codec for encoding the subtitles").
- The API can't export subtitles, edit a cue's text or style, or insert at a timecode. Appending a subtitle clip to a new timeline's empty subtitle track crashed Resolve once.

## Interchange through OTIO

Subtitles can't go to Resolve through OTIO ([X9](../deviations.md#x9-opentimelineio-interchange), [D40](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)), checked with Studio 21.1.1:

- Its OTIO export leaves subtitle tracks out, as its FCPXML 1.10 and EDL do. OpenTimelineIO has no subtitle schema; its track kinds are `Video` and `Audio`.
- Its import reads a track of kind `Subtitle` as an audio track, and refuses the whole file when a clip's media is an `.srt` file or a `MissingReference`.

So vidcut would keep its subtitle tracks in its own OTIO metadata, for its own round trip, and write each beside the timeline as an SRT named after the timeline and the track (`cut.en.srt`), in a bundle too. In Resolve: Import Subtitle, then Insert Selected Subtitles to Timeline Using Timecode (p. 1230). The way back is File > Export Subtitle, imported into vidcut. X9 would say so.

Open, for Resolve's UI since the API can't run that menu: whether the sidecar's times start at zero, as players and Resolve's embedded SubRip have them, or at the timeline's start timecode, as Resolve's TTML has them.

## Transcription without subprocesses

Two ways run whisper.cpp inside vidcut's process (AGENTS.md rule 5):

- **FFmpeg's `whisper` filter** (8.0), in an audio graph after `aresample` to 16 kHz mono float. Options in 9.0: `model`, `language` (auto), `translate` (9.0), `queue` (3 s), `use_gpu` (on), `gpu_device`, `destination`, `format` (text, srt, json), `max_len` (8.1), and Silero VAD (`vad_model` and its thresholds).
  - Times are per segment and reach only `destination`; the frame metadata (`lavfi.whisper.text`, `lavfi.whisper.duration`) holds each queued chunk's text. From 8.1, `max_len` 1 splits segments at words, whisper.cpp's way to word times. Its JSON doesn't escape the text, so SRT is the one to read. The filter runs whisper's 30 s window for each queued chunk, so files want a queue of 10–20 s, as FFmpeg's documentation says.
  - Few builds have it. Homebrew's `ffmpeg-full` (Metal on Apple Silicon) and Gyan's full build (whisper.cpp's Vulkan backend, checked here) do. Ubuntu 26.04's FFmpeg 8.0.1, BtbN's builds (the README's Windows build; whisper [disabled](https://github.com/BtbN/FFmpeg-Builds/commit/bfcf840002eb1cf68ed626657db2d250cf62e8a2) for doubling their size) and `scripts/ffmpeg/build-ffmpeg.sh` don't.
- **whisper-rs** 0.16 (Unlicense) links whisper.cpp 1.8.3 into vidcut, with features for Vulkan, CUDA, Metal and HIP. Its token timestamps give word times, and it works whatever FFmpeg a user has. The cost: vidcut's build compiles whisper.cpp with CMake (and needs the Vulkan SDK for that backend), and the binary grows.

candle (pure Rust) has a Whisper example, but its GPUs are CUDA and Metal, not AMD or Intel on Windows and Linux.

**Recommended:** whisper-rs, with Vulkan on Windows and Linux and Metal on macOS. Every user gets it, with the word times (d) needs.

- **Models** are ggml files from [Hugging Face](https://huggingface.co/ggerganov/whisper.cpp), MIT like OpenAI's weights: base 141 MiB (57 MiB quantized), small 465 MiB (181), large-v3-turbo 1.55 GiB (547 MiB); Silero VAD 864 KiB. Never in git (rule 4). vidcut has no network code, so a preference names the file, and vidcut may offer to download one into its cache dir.
- **GPU** (rule 9): whisper.cpp runs on the GPU its backend finds while `gpu.acceleration` is on. Only a GPU that gives wrong text falls back to the CPU, named in gotchas.md.
- **Speed**, unmeasured here: whisper.cpp's benchmark encodes one 30 s window in 2.4 s (tiny), 5.3 s (base) and 22.8 s (small) on a 2017 i7-8650U laptop with 4 threads, and in 0.5, 1.1 and 4.0 s on a 2021 Ryzen 7 5850U ([whisper.cpp #89](https://github.com/ggml-org/whisper.cpp/issues/89)). On a modest laptop, base runs several times faster than real time and small about real time.

## Phases

Each phase is its own decision, and each ends with the CLI, IPC and MCP doing what the GUI does (AGENTS.md rule 3).

**(a) Subtitle tracks, SRT and WebVTT, OTIO.** Large: several pull requests (model and edits, GUI, formats).
- Model: a timeline's subtitle tracks, each with a name (its language) and cues (start, end, text), sorted and not overlapping. Every edit that moves time moves cues as it moves clips: a ripple delete removes the cues in its range and shortens those it cuts through.
- Requests to add, edit, move and remove cues and tracks, and to import and export SRT and WebVTT, parsed and written in vidcut-core (plain text; vidcut-core doesn't touch FFmpeg).
- GUI: ST tracks above V1 with each cue's text, Add Subtitle at the playhead, the text edited in place; the viewer draws the shown track's cue in one style.
- OTIO as above.
- **Spec:** subtitles move into scope: tracks, cues, one style, import and export. Out: styling, regions, presets, templates, QC warnings, linking. A decision on the cue model and on how edits move cues; deviation X9 grows the SRT sidecar.
- **Risk:** a third kind of track touches every edit operation, the validation and the timeline drawing.

**(b) Subtitles in exports.** One pull request for embedded streams, several for burn-in.
- Embedded: a subtitle stream per track, its language from the track name: `mov_text` in MP4 and MOV, SubRip in Matroska, WebVTT in WebM, all FFmpeg's own encoders, muxed by libavformat. No picture work, so no GPU question.
- A separate file per track, as in (a).
- Burn-in: vidcut draws the cue as the viewer does, into a small RGBA picture, and blends it onto the frame where the frame is: a CUDA kernel beside `cuda::mix`, and per GPU the same blends [gpu-dissolves.md](gpu-dissolves.md) lists, only for frames with a cue. FFmpeg's `subtitles` filter (libass) works on frames in memory only, so it would bring GPU frames down (rule 9).
- The Export dialog gets Resolve's Deliver choices: none, burned in, embedded, separate file; the user's choice goes through as it is (rule 10).
- **Spec:** deviation X3 grows; burn-in has the viewer's one style.
- **Risk:** burn-in per GPU, as for dissolves.

**(c) Transcription.** Two or three pull requests, depending on the engine (above).
- A measurement like the audio levels ([analysis](../design/analysis.md)): words with times, cached by the file. One request makes subtitles from audio with Resolve's settings (language, characters per line, lines, gap); another returns the transcript.
- Measure with `nasa-speech-1979` (29 min of speech, [the media manifest](../../tests/media-manifest.toml)) on a modest laptop without a GPU (use case 2) before choosing the default model.
- **Spec:** Create Subtitles from Audio and transcription leave "AI tools" out of scope: Studio's features, free in vidcut, a deviation.
- **Risk:** the engine's packaging; model downloads are vidcut's first network use; quality on noisy recordings.

**(d) Cutting by transcript.** One pull request once (c) gives word times.
- A request removes words or ranges of the transcript, through the same `ops::remove_parts` that Ripple Delete Silence uses; filler words and long pauses are a decision in vidcut-core, like `analysis::silent_parts`. An agent reads the transcript, chooses, and cuts in one request.
- A transcript panel in the GUI, like Resolve's Transcription window (pp. 1004–1008), is a later choice.
- **Spec:** a deviation: Resolve's text-based editing is Studio's and lives in the Transcription window.

## Titles and effects

They stay in Resolve. Use case 1 finishes there, and vidcut already carries Resolve's titles, generators and effects through OTIO, so a recut keeps them ([X9](../deviations.md#x9-opentimelineio-interchange)). Use case 2's intro and outro are clips. A text title is burn-in's drawing plus a style editor and keyframes, which would make vidcut a finishing tool; subtitles are the one kind of text both use cases need.
