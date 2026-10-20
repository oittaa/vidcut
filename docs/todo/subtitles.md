# Subtitles

**When:** next.

Open for subtitles ([spec §8.12](../cut-page-spec.md#812-subtitles-pp-645646-702-12271248)):

- **Nemotron 3 Diarization** in Sortformer's place once transcribe.cpp releases it ([#175](https://github.com/handy-computer/transcribe.cpp/pull/175)): the same kind of model, 8 speakers instead of 4, under the OpenMDW license, for every language. Then compare it with MOSS in English: on a 3-minute press conference excerpt with seven people, MOSS told all seven apart and Sortformer four.
- **Qwen3-ASR's word times:** waiting for the aligner in transcribe.cpp ([#200](https://github.com/handy-computer/transcribe.cpp/issues/200)), Qwen3-ForcedAligner-0.6B (11 languages, up to 5 min a pass). With it, Qwen3-ASR becomes the English default.
