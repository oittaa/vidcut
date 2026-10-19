# Subtitles

**When:** next.

Subtitles made from the timeline's speech, corrected on a subtitle track and exported. Subtitle tracks, cues, subtitle files, Create Subtitles from Audio, and streams and burning in in the export are in ([spec §8.12](../cut-page-spec.md#812-subtitles-pp-645646-702-12271248)), from the GUI, the CLI, IPC and MCP; the rest below is not.

What it would take:
- **Styles:** a track's font, size, color, outline, background and margin, written to IMSC1 and drawn when it is burned in ([spec D61](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Re-timing a subtitle file to the speech:** the aligner places an imported file's text on the audio; `nasa-roman-presser-captions`, which aren't aligned with the speech, are the case to check it on. The aligner has no Finnish.
- **Cutting by transcript:** the words with their times as a request's answer, and a request that removes words. It needs word times: Parakeet Ultra's, or Qwen3-ASR's from the aligner; Whisper's only once transcribe.cpp gives them.
- **Later:** paid services (xAI, Google) behind the same interface, asking each time before audio leaves the computer.

Test media in the [manifest](../../tests/media-manifest.toml): `tos-subtitles-en`, `tos-subtitles-ja`, `tos-subtitles-he`, `tos-music-fx` (no speech), `nasa-speech-1979`, `nasa-roman-presser`, `nasa-roman-presser-captions`.

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

Peak GPU memory against piece length on the RTX 4080 SUPER (Vulkan), one model at a time over `nasa-speech-1979`: the transcribing process's own, which held nothing but the model and its work, from DXGI's `QueryVideoMemoryInfo`, local and non-local, every 10 ms. In MiB: Parakeet Ultra 786 at 10 s pieces, 896 at 30 s, 1,035 at 1 min, 1,311 at 2 min and 1,311 at 5 min; Whisper large-v3 2,114 at 30 s pieces; Qwen3-ASR 3,317 at 10 s pieces, 4,951 at 2 min and 9,855 at 5 min, lengths it would take once the aligner gives it word times.

## Open

- **The aligner in transcribe.cpp** ([#200](https://github.com/handy-computer/transcribe.cpp/issues/200)): Qwen3-ForcedAligner-0.6B (11 languages, up to 5 min a pass). Until it runs there, Qwen3-ASR gives no word times: its cues' times are estimates, its text spread over the speech, so Parakeet Ultra, with word times, is the default. Qwen3-ASR becomes the English default again once it has word times.
- **Cloud:** which services, and when.
