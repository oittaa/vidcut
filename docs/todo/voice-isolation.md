# Voice isolation

**When:** next, second in [the roadmap](../roadmap.md#what-comes-next). A Meet recording (use case 2) is still noisy after its silences are cut, and the Cut page's Voice Isolation is in the Studio version only (p. 774).

## What the Cut page does

Voice Isolation (Studio only, p. 774) takes loud sounds that aren't voice out of a recording. Its Amount mixes the source with the isolated voice: about equal at 50, and natural at 70 to 80. It runs in real time on a mono, stereo or dual-mono track, but not on live input or on more than two channels.

## What vidcut does

- **Free and local.** The model is DPDFNet's `dpdfnet2_48khz_hr` ([ceva-ip/DPDFNet](https://github.com/ceva-ip/DPDFNet)): DeepFilterNet2 with dual-path recurrent blocks, its code and weights under Apache-2.0. On the paper's multilingual low-SNR set its 16 kHz DPDFNet-2 scores a PESQ of 3.14 where DeepFilterNet3 scores 2.76, for 1.35 G MACs against 0.35 ([arXiv:2512.16420](https://arxiv.org/abs/2512.16420), Table 1). Nothing is uploaded, and there is no Python.
- **On `tract`.** `tract` 0.23.9 runs the published ONNX file as it is: 120 s of speech took 11.7 s on one thread of a Ryzen 7 9800X3D, a real-time factor of 0.098. A laptop's is unmeasured. It adds no native library, but `tract-onnx` is not small: a nearly empty program with it is 24.7 MB stripped, 4.8 MB compressed with xz, beside the 10 MB model. It is the runtime for the ONNX models that come after this one, so that size is paid once. `tract` has no GPU, and the GPU path ONNX Runtime offers is its CUDA build, for NVIDIA alone and hundreds of megabytes with NVIDIA's libraries, so it runs on the CPU.
- **Around the model.** The file holds the network and its feature normalization. vidcut adds a Vorbis window, a 960-point FFT every 480 samples at 48 kHz, the model's state (its start is in the file's metadata), the inverse FFT and overlap-add. The model is mono, so each channel has its own state.
- **What it does to speech.** On `nasa-speech-1979` at full strength, the hiss in the pauses goes to silence, and some faint speech under loud background noise goes with it.
- **Amount** from 0 to 100, as on the Cut page: that much of the isolated voice, mixed with the rest of the source. On a clip (`clip.set`) or a track (`track.set`), stored only when set. Preview and export hear the same samples (`AudioMix`).
- **Controls.** Voice Isolation is in the clip's right-click menu and on a track's strip in the Mixer, with its Amount beside it. Proposed key, free now: Alt + V on the selected clips.
- **OpenTimelineIO.** How Resolve writes Voice Isolation into an OTIO file isn't known, so the Amount goes in vidcut's own metadata, and an OTIO export warns that Resolve won't hear it.

## Open questions

- Whether the weights are built in or fetched after asking, as the speech models are ([analysis](../design/analysis.md#transcription)). The file is 10,493,337 bytes, SHA-256 `7f0575a5cec0ba4ffd8f8bd657e06d007e4ccdd955d76faab922b9d3291dc14b`, at revision `dd6818d00f50c836fed43a6243ebe49116de5964` of Ceva-IP/DPDFNet on Hugging Face.
- Which Amount keeps faint speech under loud noise, heard on short back-to-back pairs.
- The real-time factor on a laptop CPU.
