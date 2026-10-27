# macOS

**When:** zero copy for the AI processes is next. The other items come after the interchange and the smaller items ([roadmap](../roadmap.md)).

- `scripts/check.sh full` on a Mac.
- ProRes proxies on VideoToolbox checked on a Mac with ProRes hardware and on one without.
- An `.app` bundle with FFmpeg's libraries in it, signed.

## Pictures on the GPU

On macOS, VideoToolbox frames go to the viewer as Metal textures, and to the H.264 and HEVC encoders of VideoToolbox. A frame that comes down is read in place. These parts still go through the CPU:

- **The AI processes of macOS:** AI of Super Scale and AI of Retime Process get each picture as RGB from the CPU. They give RGB back.
  - On the way, the CPU converts each picture to 16-bit float RGBA and back (`frame_processor`).
  - The decoder, the processes of macOS and the compositor can all keep the picture in an IOSurface on the GPU.
  - On an M4 with macOS 27, AI of Retime Process takes 170 ms for a new pair of UHD frames. It takes 24 ms when the pair stays.
  - Thus the copies and the conversions can take most of the time. Measure each step first.
- **ProRes exports:** they use `prores_ks` on the CPU (`parts::PRORES`). `gpu_name` has no ProRes encoder of VideoToolbox.
  - vidcut already tests `prores_videotoolbox`, and proxies use it. The M4 has an engine for ProRes.
  - First check that its profiles and its quality give what the settings of the export ask for.
- **Padding and color changes:** `vt::can_fit` keeps a frame on the GPU only when its colors stay and it needs no padding. Other frames come down for the CPU.
  - A Metal compute pass on the IOSurface of the frame can pad it and convert its colors, as for the dissolves below.
- **Dissolves, burned-in subtitles, titles and placed pictures:** the frames come down while one of them shows.
  - [gpu-dissolves.md](gpu-dissolves.md), [gpu-burn-in.md](gpu-burn-in.md) and [gpu-placement.md](gpu-placement.md) plan a Metal pass for each.
- **ProRes 4444 with alpha in the viewer:** VideoToolbox gives it as AYUV64, its samples packed in one plane. The shader of the viewer doesn't read this layout, so it comes down.
