# Subtitles

**When:** next.

Subtitles made from the timeline's speech, corrected on a subtitle track and exported. The Cut page has subtitle tracks and Create Subtitles from Audio (pp. 645–646, 702, 1227–1248); the spec leaves them out ([§1](../cut-page-spec.md#1-scope)).

What it would take:
- **Transcription on the computer:** [transcribe.cpp](https://github.com/handy-computer/transcribe.cpp) (MIT, on ggml) through the `transcribe-cpp` crate, on the GPU: Vulkan, Metal, CUDA or ROCm (rule 9). FFmpeg's `whisper` filter isn't in Ubuntu's, BtbN's or vidcut's own FFmpeg builds.
- **Models**, GGUF files from Hugging Face:
  - **English, the default:** Qwen3-ASR 1.7B, with Qwen3-ForcedAligner-0.6B for word times (11 languages, up to 5 min a pass). transcribe.cpp doesn't run the aligner yet ([#200](https://github.com/handy-computer/transcribe.cpp/issues/200)).
  - **European languages:** Whisper large-v3, the most accurate of the candidates in Finnish and Spanish, 99 languages. transcribe.cpp gives it segment times only.
  - **Small GPUs:** Parakeet Ultra, 740 MB at Q8, 25 languages, token times.
  - Kept in the cache dir, which `vidcut reset` empties. Downloaded only after asking, and checked against a SHA-256, or a file the user names. Parakeet Ultra's CC BY 4.0 asks for attribution.
- **A subtitle track:** cues (start, end, text) that move with every edit as clips do; added, typed, moved, trimmed, split and joined; scriptable (rule 3).
- **Import and export:** SRT and WebVTT. On export, a file beside the output, a subtitle stream, or burned in on the GPU.
- **OTIO:** an SRT beside the OTIO, timed in the timeline's timecode. Resolve's Insert Selected Subtitles to Timeline Using Timecode (p. 1230) inserts nothing from an SRT timed from zero. An SRT beside an exported video starts at zero.
- **Cutting by transcript:** the words with their times as a request's answer, and a request that removes words. It needs word times: the aligner, or Whisper's in transcribe.cpp.
- **Later:** paid services (xAI, Google) behind the same interface, asking each time before audio leaves the computer.

Test media in the [manifest](../../tests/media-manifest.toml): `tos-subtitles-en`, `tos-subtitles-ja`, `tos-subtitles-he`, `tos-music-fx` (no speech), `nasa-speech-1979`, `nasa-roman-presser`.

## Open

- **Cloud:** which services, and when.
