# Subtitles

**When:** next.

Open for subtitles ([spec §8.12](../cut-page-spec.md#812-subtitles-pp-645646-702-12271248)):

- **Cutting by transcript:** the words with their times as a request's answer, and a request that removes words. It needs word times: Parakeet Ultra's, or Qwen3-ASR's from the aligner; Whisper's only once transcribe.cpp gives them.
- **Qwen3-ASR's word times:** waiting for the aligner in transcribe.cpp ([#200](https://github.com/handy-computer/transcribe.cpp/issues/200)), Qwen3-ForcedAligner-0.6B (11 languages, up to 5 min a pass). With it, Qwen3-ASR becomes the English default.
- **Paid services:** which ones (xAI, Google) and when, behind the same interface, asking each time before audio leaves the computer.
