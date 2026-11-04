# Super Scale

**When:** not decided.

Super Scale enlarges pictures with a better upscaler ([X91](../deviations.md#x91-super-scale-uses-the-upscaler-of-the-gpu)). The viewer and the exports enlarge with the kernel of their shader (`screen::kernel`). These parts are open:

- **Only the part that shows:** an AI upscaler enlarges the whole source picture, also when a zoom shows only a part of it.
  - It could enlarge only the shown part, with a margin for its edges. The work then follows the output size, not the zoom.
  - The upscalers read frames in order, so the part must stay the same for a run of frames. A moving zoom takes all of the parts that it shows.
  - AI Slow has attention layers. A part gives them less of the picture, so its result can change.
  - The scaler of macOS takes up to 1920x1080. A zoomed part of a UHD picture could then go to it.
- **Noise Reduction and the Enhanced modes** of Resolve (p. 433).
- **AI models:** [ai-upscaling.md](ai-upscaling.md).
