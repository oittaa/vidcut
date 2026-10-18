# Subtitles

**When:** next. The user first makes the [decisions](#decisions).

Subtitles made from the timeline's speech, corrected on a subtitle track and exported. The Cut page has subtitle tracks and Create Subtitles from Audio (pp. 645–646, 702, 1227–1248); the spec leaves them out ([§1](../cut-page-spec.md#1-scope)).

What it would take:
- **Transcription on the computer:** Whisper through whisper.cpp linked into vidcut (`whisper-rs`), on the GPU (Vulkan, Metal; rule 9). FFmpeg's `whisper` filter isn't in Ubuntu's, BtbN's or vidcut's own FFmpeg builds.
- **Models:** whisper.cpp's ggml files from Hugging Face, 60 to 570 MB, kept in the cache dir, which `vidcut reset` empties. Downloaded only after asking, and checked against a SHA-256.
- **A subtitle track:** cues (start, end, text) that move with every edit as clips do; added, typed, moved, trimmed, split and joined; scriptable (rule 3).
- **Import and export:** SRT and WebVTT. On export, a file beside the output, a subtitle stream, or burned in on the GPU.
- **OTIO:** an SRT beside the OTIO, timed in the timeline's timecode. Resolve's Insert Selected Subtitles to Timeline Using Timecode (p. 1230) inserts nothing from an SRT timed from zero. An SRT beside an exported video starts at zero.
- **Cutting by transcript:** the words with their times as a request's answer, and a request that removes words.
- **Later:** paid services (xAI, Google) behind the same interface, asking each time before audio leaves the computer.

Test media in the [manifest](../../tests/media-manifest.toml): `tos-subtitles-en`, `tos-subtitles-ja`, `tos-subtitles-he`, `tos-music-fx` (no speech), `nasa-speech-1979`, `nasa-roman-presser`.

## Decisions

1. **Engine:** whisper.cpp linked into vidcut, or FFmpeg's filter (Gyan's build on Windows, Homebrew's `ffmpeg-full` on macOS, none on Linux).
2. **Models:** an offered download, or only a file the user names.
3. **Cloud:** which services, and when.
