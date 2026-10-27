# macOS

**When:** the parts of Pictures on the GPU are next, one at a time. The other items come after the interchange and the smaller items ([roadmap](../roadmap.md)).

- `scripts/check.sh full` on a Mac.
- ProRes proxies on VideoToolbox checked on a Mac with ProRes hardware and on one without.
- An `.app` bundle with FFmpeg's libraries in it, signed.

## Pictures on the GPU

On macOS, VideoToolbox frames go to the viewer as Metal textures, and to the H.264 and HEVC encoders of VideoToolbox. A frame that comes down is read in place. These parts still go through the CPU:

- **The AI processes of macOS:** they keep VideoToolbox frames on the GPU, in and out. Frames that the CPU decodes, and frames with a turn, go through RGB.
- **ProRes 4444 exports:** they use `prores_ks` on the CPU. `prores_videotoolbox` takes 4:4:4 with alpha only as AYUV64, a layout that vidcut does not make.
- **Dissolves, burned-in subtitles, titles and placed pictures:** the frames come down while one of them shows.
  - [gpu-dissolves.md](gpu-dissolves.md), [gpu-burn-in.md](gpu-burn-in.md) and [gpu-placement.md](gpu-placement.md) plan a Metal pass for each.
- **ProRes 4444 with alpha in the viewer:** VideoToolbox gives it as AYUV64, its samples packed in one plane. The shader of the viewer doesn't read this layout, so it comes down.
