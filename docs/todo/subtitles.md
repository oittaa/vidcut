# Subtitles made from the audio

**When:** next, starting with subtitles made automatically from the audio. Before work starts, the user picks the engine and how models arrive ([decisions](#decisions)).

vidcut transcribes a timeline's speech on the user's computer with Whisper, puts the result on a subtitle track to correct, and exports it as an SRT file, as subtitle streams or burned into the picture. It serves both [use cases](../roadmap.md#use-cases): captions for a meeting recording (2), and an agent that cuts by what is said (1). Subtitles, transcription and Create Subtitles from Audio leave the spec's out-of-scope list ([§1](../cut-page-spec.md#1-scope)).

## What Resolve does

The Cut page has the Edit page's subtitles (p. 702): Timeline Actions > Add Subtitle Track and Create Subtitles from Audio, Timeline Options > Minimize Subtitle Track (pp. 645–646). The Subtitles and Closed Captioning chapter (pp. 1227–1248) has the rest:

- **Create Subtitles from Audio** (Studio only): language, caption preset, characters per line, one or two lines, gap. It writes the top subtitle track, making one if there is none (pp. 1234–1235). Transcribing clips with word timings, and editing a timeline by its text, are Studio's too (pp. 714, 995–1009).
- **Tracks:** ST1, ST2, … above the video, one per language, named for export (ISO 639 codes); only one shows at a time (p. 1228). Up to three regions per track for cues on screen together (p. 1243). Warnings for characters per line, duration and characters per second (pp. 1228–1229).
- **Cues** are clips: moved, trimmed, rolled, slipped and rippled like others (p. 1237). Added at the playhead, typed in the Inspector, found and replaced (pp. 1232–1236); fades (p. 1240); linked to a clip (p. 1242).
- **Style** per track, cue and word; presets; Fusion templates (pp. 1237–1242).
- **Import** `.srt`, `.srtx`, `.vtt`, `.xml`, `.ttml` into the Media Pool; a subtitle clip breaks into one clip per cue on a track (p. 1230). **Export** SRT or WebVTT from the File menu; on the Deliver page a file per track (IMSC1, DFXP, SRT, WebVTT), burned in, or embedded (pp. 1247–1248).

Checked with Resolve Studio 21.1.1's scripting API in a throwaway project:

- `CreateSubtitlesFromAudio` captioned 22 s of synthetic speech in 9 s: the right words, "ten percent" written "10%", cues cut at 42 characters across sentences. `GetTranscription` gives each word's start and end in frame timecode, silences as `(...)`.
- An SRT on a subtitle track becomes one clip per cue, rounded to frames; `<i>` is dropped and a line break becomes U+2028.
- Rendering: `BurnIn` drew white bold text in the lower third; `EmbeddedCaptions` gave Matroska a SubRip stream timed from zero, and failed for H.264 in MP4 and MOV; `SeparateFile` wrote IMSC1 TTML in the timeline's timecode.
- The API can't export subtitles, edit a cue or insert at a timecode. Appending a subtitle clip to a new timeline's empty subtitle track crashed Resolve once.

## Whisper in the FFmpeg builds vidcut supports

FFmpeg 8.0 added a `whisper` audio filter that runs whisper.cpp inside libavfilter. Few builds have it:

| Build | FFmpeg | `whisper` |
|---|---|---|
| Ubuntu 26.04's (the README's Linux, CI) | 8.0.1 | no |
| Debian unstable, Arch, Fedora, RPM Fusion | 9.0.2 | no |
| `scripts/ffmpeg/build-ffmpeg.sh` (vidcut's weekly build) | release, master | no |
| BtbN's GPL shared (the README's Windows, CI) | 9.0.1 | no: [disabled](https://github.com/BtbN/FFmpeg-Builds/commit/bfcf840002eb1cf68ed626657db2d250cf62e8a2) in June 2026 for doubling the build's size |
| Gyan's full and full shared (`winget install Gyan.FFmpeg`) | 9.0.2 | yes, on whisper.cpp's Vulkan backend |
| Homebrew's `ffmpeg-full` (the README's macOS, CI) | 9.0.2 | yes, on Metal |

Where it exists, it works, on the GPU too (measured below), but falls short for vidcut. Its times are per segment, written to a file it names (`destination`), with word times only from 8.1 (`max_len` 1). Its JSON doesn't escape the text. The default 3 s `queue` runs Whisper's 30 s window for every 3 s of sound, and its VAD is slow (below).

## The engine

whisper.cpp linked into vidcut, through the `whisper-rs` crate (Unlicense; 0.16 builds whisper.cpp 1.8.3), keeps rule 5 and reaches every user, whatever FFmpeg they have:

- Word times from its token timestamps, the language detected or given, and translation into English.
- GPU first (rule 9): its Vulkan backend on Windows and Linux (NVIDIA, AMD and Intel), Metal on macOS, the CPU only where no GPU works or `gpu.acceleration` is off.
- Its cost: vidcut's build compiles whisper.cpp with CMake, and the Vulkan backend needs the Vulkan SDK there and in CI. The binary grows by the GPU shaders.

Linking a system libwhisper, as vidcut links FFmpeg, would work on Linux and macOS (Ubuntu 26.04's universe has 1.8.3, Arch and Homebrew 1.9.5), but whisper.cpp's own Windows downloads are CPU, BLAS and CUDA builds: AMD and Intel GPUs on Windows would transcribe on the CPU, and the version would differ by system. Candle, in pure Rust, runs Whisper on CUDA and Metal only.

## Models, speed and languages

Models are ggml files from [Hugging Face](https://huggingface.co/ggerganov/whisper.cpp), MIT like OpenAI's weights. Measured on a Windows desktop (Ryzen 7 9800X3D, RTX 4080 SUPER) with Gyan's FFmpeg 9.0.2 `whisper` filter (whisper.cpp on Vulkan), English, a 20 s queue, no VAD, over `nasa-speech-1979` (29 min of speech, [the media manifest](../../tests/media-manifest.toml)), loading the model included; the CPU over its first 5 min:

| Model | File | RTX 4080 SUPER | Ryzen 7 9800X3D, 8 threads | 4 threads |
|---|---|---|---|---|
| `base` | 148 MB (60 MB quantized) | 29 min in 9.8 s, 179× real time | 10× | 11× |
| `small` | 488 MB (190 MB) | 17.9 s, 98× | 4.4× | 5.3× |
| `large-v3-turbo`, quantized `q5_0` | 574 MB (1.6 GB full) | 13.5 s, 130× | 2.4× | 1.7× |

- `large-v3-turbo` heard the most: at 20:16, "we used to see them early in the morning", which `base` heard as "see Merlin the morning". On the GPU it took 1.4 GB of video memory. All three marked music and noises in brackets (`[Music]`, `[phone ringing]`).
- **A modest laptop** is slower than this CPU's 4 threads. whisper.cpp's benchmark encodes a 30 s window with `base` in 5.3 s on a 2017 i7-8650U and 1.1 s on a 2021 Ryzen 7 5850U, with `small` in 22.8 and 4.0 s ([whisper.cpp #89](https://github.com/ggml-org/whisper.cpp/issues/89)). So `base` takes minutes for a meeting on a laptop CPU, `small` about the meeting's length on an older one, and `large-v3-turbo` wants a GPU. Phase 1 measures on a laptop.
- **VAD:** a 885 KB [Silero model](https://huggingface.co/ggml-org/whisper-vad) finds speech. Whisper needs it: on music and effects alone it invents text ([testing](#testing)).
  - FFmpeg's filter uses it only to choose where to cut, and still transcribes a full queue without speech. On the music it changed nothing.
  - It is slow there too: 2 min took 89 s instead of 1.7 s on the GPU. The filter runs VAD again over its whole buffer every 0.6 s, and transcribes each stretch between pauses with a full 30 s window.
  - vidcut would run it once per file, and transcribe only what it finds. Phase 1 checks that it finds no speech in `tos-music-fx`.

Delivering models:

- **Where they live:** a `models` folder in vidcut's cache dir, which `vidcut reset` keeps unless asked, like presets. Never in git (rule 4).
- **First run:** vidcut asks before downloading, naming the model, its size and its source, and checks the file against a SHA-256 it carries. It would be vidcut's first network code. Or the user names a file they have, which offline computers need anyway. Bundling would add 60 to 570 MB to every download of vidcut, for a feature not everyone uses.
- **Default:** `large-v3-turbo` where a GPU transcribes, `base` on the CPU, both changeable in Preferences and per request.
- **Languages:** the multilingual models take 99 languages, detected per file or given; quality varies widely by language, and OpenAI publishes the word error rate for each. `large-v3-turbo` doesn't translate. Resolve's list is 14 languages built in and 45 in an optional pack (p. 1235).

## The subtitle track

- **Model:** a timeline's subtitle tracks, each with a name (its language) and cues (start, end, text), sorted and not overlapping, in the project file. One track shows in the viewer, as in Resolve.
- **Following edits:** every edit that moves time moves cues as it moves clips; a ripple delete removes the cues in its range and shortens those it cuts through. Ripple Delete Silence and cutting by transcript keep subtitles in step for free.
- **Editing:** add a cue at the playhead, type its text in place, move and trim it on the track, split and join, find and replace. Out, for now: styling beyond one style, regions, presets, templates, linking, QC warnings.
- **Import and export:** SRT and WebVTT, parsed and written in vidcut-core.
- Every action is a request, so the CLI, IPC and MCP do what the GUI does (rule 3).

## Export

- **Separate file** per track, SRT or WebVTT, beside the output.
- **Embedded:** a subtitle stream per track, its language from the track's name: `mov_text` in MP4 and MOV, SubRip in Matroska, WebVTT in WebM, all FFmpeg's own encoders. No picture work.
- **Burned in:** vidcut draws the cue as the viewer does, into a small RGBA picture, and blends it onto the frame where the frame is, only for frames with a cue: a CUDA kernel beside `cuda::mix`, and on the other GPUs the blends [gpu-dissolves.md](gpu-dissolves.md) lists. FFmpeg's `subtitles` filter works on frames in memory only, so it would bring GPU frames down (rule 9).
- The Export dialog takes Resolve's Deliver choices (none, separate file, embedded, burned in), and the user's choice goes through as it is (rule 10).

## OTIO

Subtitles can't go to Resolve through OTIO ([X9](../deviations.md#x9-opentimelineio-interchange), [D40](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)), checked with Studio 21.1.1:

- Its OTIO export leaves subtitle tracks out, as its FCPXML 1.10 and EDL do. OpenTimelineIO has no subtitle schema; its track kinds are `Video` and `Audio`.
- Its import reads a track of kind `Subtitle` as an audio track, and refuses the whole file when a clip's media is an `.srt` file or a `MissingReference`.

So vidcut keeps its subtitle tracks in its own OTIO metadata for its own round trip, and writes each beside the timeline as an SRT (`cut.en.srt`), in bundles too. Checked by hand in Resolve's menus:

- **For Resolve,** the SRT beside the OTIO is timed in the timeline's timecode (`01:00:01,000`), as Resolve's File > Export > Subtitle writes it. Import Subtitle, then Insert Selected Subtitles to Timeline Using Timecode (p. 1230), puts each cue back in place. From an SRT timed from zero it inserts nothing and says nothing.
- **For players,** an SRT beside an exported video starts at zero, at the export's first frame.
- **Reading an SRT** takes the timeline's start timecode off when every cue is at or after it. `<b>`, `<i>` and `<u>` are styling, dropped while vidcut has one style; Resolve's export wraps every cue in `<b>`.

## Cutting by transcript

The transcript, words with timeline times, is a request's answer. Another removes words or ranges through `ops::remove_parts`, as Ripple Delete Silence does; filler words and long pauses are a decision in vidcut-core, like `analysis::silent_parts`. An agent reads, chooses and cuts in one request. A transcript panel like Resolve's Transcription window (pp. 1004–1008) can come later.

## Cloud transcription, later

Transcription sits behind one interface: the timeline's speech in, words with times and the language out. Local Whisper is the first backend; a paid service is another, its words made into cues by the same rules. The track, editing, export and cutting by transcript don't know which backend made the words.

| Checked 2026-10-18 | xAI (SpaceX) | Google Cloud Speech-to-Text V2 |
|---|---|---|
| Model | `grok-voice-transcribe-2.0` | `chirp_3` |
| Word times | always, to 10 ms; speakers with `diarize` | `enable_word_time_offsets`, called experimental; with `chirp_3` in batch only, at "some transcription degradation" |
| Languages | 38+, detected, following switches mid-recording | about 111 locales (29 generally available), the main one detected |
| Limits | one request per file, up to 500 MB | 1 min inline; longer audio goes through Cloud Storage, and `chirp_3` with word times takes 20 min per file |
| Price per hour | $0.10 (streaming $0.20) | $0.96 standard, $0.18 dynamic batch |
| Retention | kept 30 days for abuse review, not trained on without permission; Zero Data Retention per team | not stored when synchronous, batch results about 5 days; trained on only with data logging, which is off by default |
| Credentials | an API key | a Google Cloud project with billing, a service account, and a bucket |

Sources: [xAI's speech to text](https://docs.x.ai/developers/model-capabilities/audio/speech-to-text), [models and prices](https://docs.x.ai/developers/models), [security](https://docs.x.ai/developers/faq/security); [Google's Chirp 3](https://docs.cloud.google.com/speech-to-text/v2/docs/chirp_3-model), [pricing](https://cloud.google.com/speech-to-text/pricing), [quotas](https://docs.cloud.google.com/speech-to-text/v2/quotas), [data usage](https://docs.cloud.google.com/speech-to-text/docs/data-usage-faq). xAI fits a desktop app better: one key, one request, word times, the lowest price. Others with word times: Deepgram Nova-3 ($0.26/h), AssemblyAI Universal-2 ($0.15/h), ElevenLabs Scribe v2 ($0.22/h), OpenAI's `whisper-1` ($0.36/h, 25 MB files; its newer models give no word times).

Each service needs:
- **Its key** in the system's keychain (Windows Credential Manager, macOS Keychain, the Secret Service on Linux) or an environment variable for scripts, never in the project file, `preferences.toml` or a log.
- **Consent each time the audio leaves the computer:** the GUI names the service and the cost (minutes times price) and asks, and a project can remember the yes; the CLI and MCP take the service as an argument with no default.
- **The same result:** words with times, made into cues as Whisper's are.

## Testing

The [media manifest](../../tests/media-manifest.toml) holds Tears of Steel's subtitles and soundtrack beside `tos-720p-mov`:

- `tos-subtitles-en`: the reference text for transcription accuracy against the movie, and a real SRT to import (ASCII, CRLF). Its 76 cues are timed from the movie's start, rounded to half seconds; speech starts a median 0.3 s before a cue, most within a second (measured against Whisper's word times).
- `tos-subtitles-ja`, `tos-subtitles-he`: UTF-8 without a BOM, CJK and right-to-left text in the viewer and burned in. The Hebrew file adds 8 cues for titles and credits.
- `tos-music-fx`: music and effects without dialogue, the movie's length. Transcription must find no speech. Through FFmpeg's filter, `large-v3-turbo` invented 53 cues on it ("The first time I was in the middle of the night…"), and the same 9 in its first 3 min with the filter's VAD on.
- `nasa-speech-1979`: speed, on a GPU and on a laptop's CPU.

Unit tests take short SRT and WebVTT written in the test (rule 8); the media are for end-to-end tests.

## Phases

1. **Subtitles from the audio.** The engine, models and their download; the subtitle track with cues that follow edits, shown on the timeline and in the viewer, their text editable; Create Subtitles from Audio with Resolve's settings (language, characters per line, lines, gap); SRT and WebVTT export, and embedded streams. Several pull requests. Risks: the build and CI with whisper.cpp and the Vulkan SDK on three systems; the first network code; a third kind of track touching every edit operation.
2. **Editing and interchange:** the rest of cue editing, SRT and WebVTT import, the OTIO sidecar. Two or three pull requests.
3. **Burned in on the GPU**, per GPU as for dissolves. Several pull requests.
4. **Cutting by transcript** for agents, then the GUI. One or two.
5. **Cloud backends**, each behind the same interface. One per service.

**Spec and deviations:** §1 moves subtitles and transcription into scope; a decision on the cue model and on how edits move cues; deviations for Create Subtitles from Audio and text-based editing (Studio's in Resolve, free in vidcut), for the one style, and X9 for the SRT sidecar.

## Decisions

1. **Engine:** whisper.cpp built into vidcut (recommended), or FFmpeg's filter, which means Gyan's build on Windows instead of BtbN's, Homebrew's `ffmpeg-full` on macOS, and no distribution's FFmpeg on Linux.
2. **Models:** an offered download (recommended), or only a file the user names.
3. **Cloud:** which services, and when.

## Titles and effects

They stay in Resolve. Use case 1 finishes there, and vidcut already carries Resolve's titles, generators and effects through OTIO, so a recut keeps them ([X9](../deviations.md#x9-opentimelineio-interchange)). Use case 2's intro and outro are clips. A text title is burn-in's drawing plus a style editor and keyframes, which would make vidcut a finishing tool; subtitles are the one kind of text both use cases need.
