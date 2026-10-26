# Model catalog: what is still open

**When:** when a maker publishes the numbers.

`crates/vidcut-core/src/catalog.rs` holds each model and speech service that vidcut uses. `vidcut models list` prints it, and `vidcut models check` checks a model ([tools.md](../tools.md)).

- **GPU memory of the speech models:** Parakeet Ultra, Qwen3-ASR and Whisper large-v3 have `Source::Unrecorded`.
  - On 2026-10-25, no maker gave the GPU memory of the Q8_0 GGUF files that vidcut uses.
  - These were read: the GGUF cards of handy-computer, the docs of transcribe.cpp, and the cards of Moondream, NVIDIA, Qwen and OpenAI.
  - OpenAI gives about 10 GB for Whisper large, for its own model, not for the GGUF.
