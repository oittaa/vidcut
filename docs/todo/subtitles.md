# Subtitles

**When:** next.

Open for subtitles ([spec §8.12](../cut-page-spec.md#812-subtitles-pp-645646-702-12271248)):

- **Nemotron 3 Diarization** in Sortformer's place, when transcribe.cpp releases it ([#175](https://github.com/handy-computer/transcribe.cpp/pull/175)).
  - It is the same kind of model as Sortformer, for every language.
  - It finds 8 speakers instead of Sortformer's 4.
  - Its license is OpenMDW.
  - Then compare it with MOSS in English.
  - On a 3-minute press conference excerpt with seven people, MOSS tells all seven apart, and Sortformer four.
- **Qwen3-ASR's word times:**
  - They need the aligner in transcribe.cpp ([#200](https://github.com/handy-computer/transcribe.cpp/issues/200)): Qwen3-ForcedAligner-0.6B.
  - The aligner has 11 languages, and takes up to 5 min in a pass.
  - With the aligner, Qwen3-ASR becomes the English default.
