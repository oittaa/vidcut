# Subtitles

**When:** next.

Subtitles made from the timeline's speech, corrected on a subtitle track and exported. Subtitle tracks, cues and subtitle files are in ([spec §8.12](../cut-page-spec.md#812-subtitles-pp-645646-702-12271248)), from the CLI, IPC and MCP; Create Subtitles from Audio (p. 1234) and the rest below are not.

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
- **The GUI:** subtitle tracks on the timeline, cues drawn on them and in the viewer, moved, trimmed and typed with the mouse and keys; Add Subtitle Track, Add Subtitle, Import Subtitle and Export Subtitle in the menus; subtitle files in the Export dialog.
- **Styles:** a track's font, size, color, outline, background and margin, written to IMSC1.
- **In the export:** a subtitle stream (mov_text in MP4 and MOV, SubRip in Matroska, WebVTT in WebM), or burned in on the GPU.
- **Re-timing a subtitle file to the speech:** the aligner places an imported file's text on the audio; `nasa-roman-presser-captions`, which aren't aligned with the speech, are the case to check it on. The aligner has no Finnish.
- **Cutting by transcript:** the words with their times as a request's answer, and a request that removes words. It needs word times: the aligner, or Whisper's in transcribe.cpp.
- **Later:** paid services (xAI, Google) behind the same interface, asking each time before audio leaves the computer.

Test media in the [manifest](../../tests/media-manifest.toml): `tos-subtitles-en`, `tos-subtitles-ja`, `tos-subtitles-he`, `tos-music-fx` (no speech), `nasa-speech-1979`, `nasa-roman-presser`, `nasa-roman-presser-captions`.

## Measured

transcribe.cpp 0.4.0's prebuilt bundles, over the first 10 min of `nasa-speech-1979`, Q8_0 models; the second run of two. `scripts/stt-speed.py` runs it again ([tools](../tools.md)).

| Device | Parakeet Ultra | Whisper large-v3 | Qwen3-ASR 1.7B, 30 s pieces |
|---|---|---|---|
| RTX 4080 SUPER, Vulkan | 1.8 s | 9.4 s | 12.0 s |
| RTX 4080 SUPER, CUDA 12 | 19.3 s | 17.0 s | 10.6 s |
| Apple M4, Metal | 12.1 s | 70.9 s | 74.3 s |
| Ryzen 7 9800X3D, 8 cores | 21.4 s | 142.1 s | 109.5 s |
| Apple M4, CPU | 26.2 s | 202.0 s | 137.1 s |
| Radeon Graphics (Ryzen 7 9800X3D), Vulkan | 30.6 s | 393.6 s | 213.3 s |

GPU memory: the RTX 4080 SUPER's at the most, Vulkan / CUDA, is 1.2 / 1.6 GB for Parakeet Ultra, 2.2 / 2.4 GB for Whisper large-v3 and 4.4 / 3.8 GB for Qwen3-ASR; the M4 holds 0.7, 1.6 and 2.8 GB after the runs.

## Open

- **Cloud:** which services, and when.
