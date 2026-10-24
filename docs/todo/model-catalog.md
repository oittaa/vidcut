# Model catalog: one check and a models command

**When:** next, after the catalog file is on master.

`crates/vidcut-core/src/catalog.rs` holds each model and speech service that vidcut uses: its files, languages, limits and price. Two steps are still to do.

## 1. One check of the limits

- One function in the catalog takes a job and gives its refusals and its warnings.
- The CLI, the GUI and MCP call it before the work starts. No other code tests a limit.
- What vidcut does for each kind of limit:
  - `Kind::Hard`: vidcut refuses before the work starts, and tells why.
  - `Kind::Cap`: the work runs, but the result has this maximum. vidcut tells the user before the start.
  - `Kind::Soft`: the work runs. vidcut warns where the user sets the value and in the report.
- The owner's decisions:
  - A speech model that does not load on the GPU is soft. vidcut warns and uses the CPU.
  - Gemini with more than 8 speakers is soft.
- The check does not read these limits of the catalog yet:
  - xAI's 100 words to listen for, 50 characters each, and its 8 channels. Now xAI itself refuses them.
  - The HDR limit of the two AI upscalers. Now `upscale::unfit` and `flashvsr::unfit` test it.

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
