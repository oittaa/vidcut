# macOS

**When:** zero copy for the AI processes is next. The other items come after the interchange and the smaller items ([roadmap](../roadmap.md)).

- `scripts/check.sh full` on a Mac.
- ProRes proxies on VideoToolbox checked on a Mac with ProRes hardware and on one without.
- An `.app` bundle with FFmpeg's libraries in it, signed.

## Pictures on the GPU

On macOS, VideoToolbox frames go to the viewer as Metal textures, and to the H.264 and HEVC encoders of VideoToolbox. A frame that comes down is read in place. These parts still go through the CPU:

- **The AI processes of macOS:** AI of Retime Process takes VideoToolbox frames on the GPU. These parts still go through the CPU:
  - The frame that AI of Retime Process makes comes back as RGB, and goes on in memory.
  - AI of Super Scale gets each picture as RGB from the CPU, and gives RGB back. Its scaling takes most of the time, but the copies take memory and CPU time.
  - The decoder, the processes of macOS and the compositor can all keep the picture in an IOSurface on the GPU.
- **ProRes exports:** they use `prores_ks` on the CPU (`parts::PRORES`). `gpu_name` has no ProRes encoder of VideoToolbox.
  - vidcut already tests `prores_videotoolbox`, and proxies use it. The M4 has an engine for ProRes.
  - First check that its profiles and its quality give what the settings of the export ask for.
- **Padding and color changes:** `vt::can_fit` keeps a frame on the GPU only when its colors stay and it needs no padding. Other frames come down for the CPU.
  - A Metal compute pass on the IOSurface of the frame can pad it and convert its colors, as for the dissolves below.
- **Dissolves, burned-in subtitles, titles and placed pictures:** the frames come down while one of them shows.
  - [gpu-dissolves.md](gpu-dissolves.md), [gpu-burn-in.md](gpu-burn-in.md) and [gpu-placement.md](gpu-placement.md) plan a Metal pass for each.
- **ProRes 4444 with alpha in the viewer:** VideoToolbox gives it as AYUV64, its samples packed in one plane. The shader of the viewer doesn't read this layout, so it comes down.
