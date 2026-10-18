# Subtitles

**When:** next.

Subtitles made from the timeline's speech, corrected on a subtitle track and exported. The Cut page has subtitle tracks and Create Subtitles from Audio (pp. 645–646, 702, 1227–1248); the spec leaves them out ([§1](../cut-page-spec.md#1-scope)).

What it would take:
- **Transcription on the computer:** [transcribe.cpp](https://github.com/handy-computer/transcribe.cpp) (MIT, on ggml) built into vidcut through the `transcribe-cpp` crate, on the GPU (rule 9): Vulkan on Windows and Linux, Metal on macOS.
  - No CUDA. Its backend and NVIDIA's CUDA 12 runtime and cuBLAS add about 930 MB, and it was slower than Vulkan for two of the three models ([measured](#measured)).
  - transcribe.cpp's Windows CPU and Vulkan DLLs are 53.6 MB unpacked, 41.6 MB of it the Vulkan backend.
  - FFmpeg's `whisper` filter isn't in Ubuntu's, BtbN's or vidcut's own FFmpeg builds.
- **Audio in pieces cut at pauses:** transcribe.cpp's Silero VAD finds the speech. A piece ends in a silence, with a little padding, and stays under a maximum: the aligner takes 5 min, and Qwen3-ASR ran a 16 GB GPU out of memory on 10 min in one pass. A long stretch without a pause is cut at its quietest point. Each piece's times shift back to the timeline's.
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

## Measured

transcribe.cpp 0.4.0's prebuilt Windows bundles, over the first 10 min of `nasa-speech-1979`, Q8_0 models; the second run of two. `scripts/stt-speed.py` runs it again ([tools](../tools.md)).

| Model | RTX 4080 SUPER, Vulkan | RTX 4080 SUPER, CUDA 12 | Radeon Graphics (Ryzen 7 9800X3D), Vulkan | Ryzen 7 9800X3D, 8 cores |
|---|---|---|---|---|
| Parakeet Ultra | 1.8 s | 19.3 s | 30.6 s | 21.4 s |
| Whisper large-v3 | 9.4 s | 17.0 s | 393.6 s | 142.1 s |
| Qwen3-ASR 1.7B, 30 s pieces | 12.0 s | 10.6 s | 213.3 s | 109.5 s |

The RTX 4080 SUPER's memory at the most, Vulkan / CUDA: Parakeet Ultra 1.2 / 1.6 GB, Whisper large-v3 2.2 / 2.4 GB, Qwen3-ASR 4.4 / 3.8 GB.

## Open

- **Cloud:** which services, and when.
