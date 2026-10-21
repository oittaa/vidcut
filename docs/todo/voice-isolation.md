# Voice isolation

**When:** next, second in [the roadmap](../roadmap.md#what-comes-next). A Meet recording (use case 2) is still noisy after its silences are cut, and the Cut page's Voice Isolation is in the Studio version only (p. 774).

## What the Cut page does

Voice Isolation (Studio only, p. 774) takes loud sounds that aren't voice out of a recording. Its Amount mixes the source with the isolated voice: about equal at 50, and natural at 70 to 80. It runs in real time on a mono, stereo or dual-mono track, but not on live input or on more than two channels.

## What vidcut does

- **Free and local.** The model is DeepFilterNet3, dual-licensed MIT or Apache-2.0 (`libDF/Cargo.toml` in [Rikorose/DeepFilterNet](https://github.com/Rikorose/DeepFilterNet)). Its paper reports a real-time factor of 0.19 on one thread of a notebook CPU ([arXiv:2305.08227](https://arxiv.org/abs/2305.08227)). It runs in Rust on `tract`, on the CPU: nothing is uploaded, and there is no Python.
- **The crate.** The `deep_filter` on crates.io is 0.2.5, from 2022, and the repository's `libDF` is 0.5.7-pre, so vidcut pins a git revision. The repository's last commit is from 2024-10-17.
- **Amount** from 0 to 100, as on the Cut page, on a clip (`clip.set`) or a track (`track.set`), and stored only when set. Preview and export hear the same samples (`AudioMix`). There is one model, with no lighter one swapped in behind the user's back.
- **Controls.** Voice Isolation is in the clip's right-click menu and on a track's strip in the Mixer, with its Amount beside it. Proposed key, free now: Alt + V on the selected clips.
- **OpenTimelineIO.** How Resolve writes Voice Isolation into an OTIO file isn't known, so the Amount goes in vidcut's own metadata, and an OTIO export warns that Resolve won't hear it.

## Open questions

- Whether the weights are built in with the crate's `default-model` feature or fetched after asking, as the speech models are ([analysis](../design/analysis.md#transcription)), pinned by SHA-256. `DeepFilterNet3_onnx.tar.gz` is 7,983,136 bytes, and the low-latency `DeepFilterNet3_ll_onnx.tar.gz` 36,359,660.
- Which of the two, by the real-time factor measured on a laptop.
- What playback does on a computer too slow to run it live.
