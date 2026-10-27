# macOS

**When:** after the interchange and the smaller items ([roadmap](../roadmap.md)).

To do:

- Test ProRes proxies on a Mac with ProRes hardware and on a Mac without it.
- Make a signed `.app` bundle that includes FFmpeg's libraries.

## Work on the CPU

Most frames stay on the GPU from the decoder to the viewer or the encoder. These cases use the CPU:

- **Super Scale and Retime Process on frames that the CPU decodes.** The CPU converts these frames to RGB first. Frames that VideoToolbox decodes stay on the GPU.
- **Placed pictures.** The CPU draws a moved, zoomed or cropped picture. The frame under it stays on the GPU ([gpu-placement.md](gpu-placement.md)).
- **A timeline color space that is not the color space of the export.**
  - The frames of the timeline are in memory (`Output::Fitted`).
  - The CPU mixes these frames.
  - NVENC keeps these frames on the GPU, in a P010 pool.
  - VideoToolbox has no such pool.
