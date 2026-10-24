# Model catalog: a models command and the sources

**When:** next.

`crates/vidcut-core/src/catalog.rs` holds each model and speech service that vidcut uses: its files, languages, limits and price. `catalog::check` tests the limits. These steps are still to do.

## 1. Limits that the check does not read yet

- Sortformer's warning uses the most that transcribe.cpp reports at load, not the catalog. The check command of step 2 compares the two.
- For multichannel xAI, a file with more than 8 channels is heard with 8 (`channels_of`). The owner decides if vidcut refuses it instead.

## 2. A models command and the update procedure

- A models command prints the catalog, also as JSON. It is the one place to audit the downloads and the limits.
- A check command downloads the files of a model, checks their SHA-256, and loads the model.
  - It compares what transcribe.cpp reports with the catalog: the longest audio, the kind of times and the most speakers.
  - A difference is an error that names the field.
- The update procedure goes into [tools.md](../tools.md):
  1. Change only the entry of the model in the catalog.
  2. Take the sizes and SHA-256 from the release data of GitHub or Hugging Face.
  3. Take each limit from its source, and set the date of the check.
  4. Run the check command, then the commit gate.
  5. The pull request lists each changed field: the old value, the new value and the source.
- The help text of the schema repeats some numbers: Gemini's 30 minutes and 8 speakers, and xAI's 500 MB.
  - With the models command, the help text names the model and leaves the numbers to the command.

## 3. Sources still to record

These limits have `Source::Unrecorded`. Each needs its source from the maker of the model, and the date of the check:

- The GPU memory of Parakeet Ultra, Qwen3-ASR and Whisper large-v3.
- xAI's request size, words to listen for and channels.
- The scale of NanoVSR and of FlashVSR+.
