# Subtitles

**When:** next.

Subtitles made from the timeline's speech, corrected on a subtitle track and exported. The Cut page has subtitle tracks and Create Subtitles from Audio (pp. 645–646, 702, 1227–1248); the spec leaves them out ([§1](../cut-page-spec.md#1-scope)).

What it would take:
- **Transcription on the computer:** [transcribe.cpp](https://github.com/handy-computer/transcribe.cpp) (MIT, on ggml) built into vidcut through the `transcribe-cpp` crate, on the GPU (rule 9): Vulkan on Windows and Linux, Metal on macOS.
  - No CUDA. Its backend and NVIDIA's CUDA 12 runtime and cuBLAS add about 930 MB, and it was slower than Vulkan for two of the three models ([measured](#measured)).
  - transcribe.cpp's Windows CPU and Vulkan DLLs are 53.6 MB unpacked, 41.6 MB of it the Vulkan backend.
  - FFmpeg's `whisper` filter isn't in Ubuntu's, BtbN's or vidcut's own FFmpeg builds.
- **Audio in pieces cut at pauses:** transcribe.cpp's Silero VAD finds the speech, on the CPU: transcribe.cpp's docs call its GPU path less accurate (rule 9's exception). A piece ends in a silence, with a little padding, and stays under a maximum: the aligner takes 5 min, and Qwen3-ASR ran a 16 GB GPU out of memory on 10 min in one pass. A long stretch without a pause is cut where vidcut finds it quietest, not at the cap where the VAD's own segments would end. Each piece's times shift back to the timeline's.
- **Models**, GGUF files from Hugging Face:
  - **English, the default:** Qwen3-ASR 1.7B, with Qwen3-ForcedAligner-0.6B for word times (11 languages, up to 5 min a pass). transcribe.cpp doesn't run the aligner yet ([#200](https://github.com/handy-computer/transcribe.cpp/issues/200)).
  - **European languages:** Whisper large-v3, the most accurate of the candidates in Finnish and Spanish, 100 languages. transcribe.cpp gives it segment times only; asked for word times, it answers that the model has none.
  - **Voice activity:** transcribe.cpp's Silero VAD model, downloaded with the first model.
  - **Small GPUs:** Parakeet Ultra, 740 MB at Q8, 25 languages, word times.
  - Kept in the cache dir, which `vidcut reset` empties. Downloaded only after asking, and checked against a SHA-256, or a file the user names. Parakeet Ultra's CC BY 4.0 asks for attribution.
- **A subtitle track:** cues (start, end, text) that move with every edit as clips do; added, typed, moved, trimmed, split and joined; scriptable (rule 3).
- **Import and export:** SRT and WebVTT. On export, a file beside the output, a subtitle stream, or burned in on the GPU.
- **OTIO:** an SRT beside the OTIO, timed in the timeline's timecode. Resolve's Insert Selected Subtitles to Timeline Using Timecode (p. 1230) inserts nothing from an SRT timed from zero. An SRT beside an exported video starts at zero.
- **Re-timing a subtitle file to the speech:** the aligner places an imported file's text on the audio; `nasa-roman-presser-captions`, which aren't aligned with the speech, are the case to check it on. The aligner has no Finnish.
- **Cutting by transcript:** the words with their times as a request's answer, and a request that removes words. It needs word times: Parakeet Ultra's, or Qwen3-ASR's from the aligner; Whisper's only once transcribe.cpp gives them.
- **Later:** paid services (xAI, Google) behind the same interface, asking each time before audio leaves the computer.

Test media in the [manifest](../../tests/media-manifest.toml): `tos-subtitles-en`, `tos-subtitles-ja`, `tos-subtitles-he`, `tos-music-fx` (no speech), `nasa-speech-1979`, `nasa-roman-presser`.

## Measured

transcribe.cpp 0.4.0's prebuilt bundles, over the first 10 min of `nasa-speech-1979`, Q8_0 models; the second run of two. `scripts/stt-speed.py` ([tools](../tools.md)) repeats every row but CUDA's, which came from transcribe.cpp's CUDA bundle with NVIDIA's CUDA 12 runtime, neither of which it fetches.

| Device | Parakeet Ultra | Whisper large-v3 | Qwen3-ASR 1.7B, 30 s pieces |
|---|---|---|---|
| RTX 4080 SUPER, Vulkan | 1.8 s | 9.4 s | 12.0 s |
| RTX 4080 SUPER, CUDA 12 | 19.3 s | 17.0 s | 10.6 s |
| Apple M4, Metal | 12.1 s | 70.9 s | 74.3 s |
| Ryzen 7 9800X3D, 8 cores | 21.4 s | 142.1 s | 109.5 s |
| Apple M4, CPU | 26.2 s | 202.0 s | 137.1 s |
| Radeon Graphics (Ryzen 7 9800X3D), Vulkan | 30.6 s | 393.6 s | 213.3 s |

GPU memory: the RTX 4080 SUPER's at the most, sampled with `nvidia-smi` during the runs, Vulkan / CUDA, is 1.2 / 1.6 GB for Parakeet Ultra, 2.2 / 2.4 GB for Whisper large-v3 and 4.4 / 3.8 GB for Qwen3-ASR; the M4 holds 0.7, 1.6 and 2.8 GB after the runs, the figure `scripts/stt-speed.py` reports.

## Open

- **Cloud:** which services, and when.
