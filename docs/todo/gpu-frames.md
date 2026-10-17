# GPU frames: one pipeline

**When:** before another GPU path is added: QSV from GPU memory, or Vulkan Video decoding ([own-player.md](own-player.md)).

**The rule:** a frame stays in GPU memory from the decoder to the encoder or the display wherever that is possible, even where the CPU would be a little quicker (AGENTS.md rule 9).

**Today:** five paths, each added for one GPU and one job: CUDA (`cuda::CudaPictures`), VA-API, Direct3D 11 and VideoToolbox (`hw::Fitter`, `hw::Shrinker`), Vulkan; the player's kept frames (`hw::Keeper`); the CPU's (`export::normalize`). Where a frame goes is chosen per frame in `route.rs`, and the encoder's place per job by its name.

**The design:** one backend for each place (memory, CUDA, VA-API, Direct3D 11, VideoToolbox, Vulkan) with the same functions: `fit` (turn, scale, convert, pad, with `can_fit` saying what it can), `down` and `up`, `keep` for the player, `encoder_frames` for an encoder on its device. A planner, a pure function of the frame, the job and the backends' capabilities, prefers everything where the decoder put the frame, then fitting there and copying the small picture down, then the CPU. Capabilities are tested once for each GPU and FFmpeg and kept with `encoders.json`. Every job, exports, proxies, tiles, renders, thumbnails, measurements, goes through the one engine.

**Checked by:** unit tests of the planner, each one input and its route; each backend's pictures against the CPU's on the barcode fixtures; a script that runs the jobs on the GPU in use and compares the result's `copies` with the route.
