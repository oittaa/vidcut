# macOS

**When:** the parts of Pictures on the GPU are next, one at a time. The other items come after the interchange and the smaller items ([roadmap](../roadmap.md)).

- `scripts/check.sh full` on a Mac.
- ProRes proxies on VideoToolbox checked on a Mac with ProRes hardware and on one without.
- An `.app` bundle with FFmpeg's libraries in it, signed.

## Pictures on the GPU

On macOS, VideoToolbox frames go to the viewer as Metal textures, and to the H.264 and HEVC encoders of VideoToolbox. A frame that comes down is read in place. These parts still go through the CPU:

- **The AI processes of macOS:** they keep VideoToolbox frames on the GPU, in and out. Frames that the CPU decodes, and frames with a turn, go through RGB.
- **ProRes 4444 exports:** they use `prores_ks` on the CPU. `prores_videotoolbox` takes 4:4:4 with alpha only as AYUV64, a layout that vidcut does not make.
- **Scaling:** `scale_vt` and the pixel transfer of VideoToolbox keep less detail than lanczos on the CPU.
  - A test on the owner's M4: AI of Super Scale enlarged 960x540 into 1080p. Against the real frames, luma PSNR was 32.9 dB on the GPU path and 37.1 dB on the CPU path.
  - The loss is in the step that makes the 3840x2160 picture of macOS's scaler smaller. Chroma differs by less than 2 dB.
  - A Metal pass with lanczos (`MPSImageLanczosScale`) on the IOSurfaces of the frames can scale them on the GPU.
- **Padding and color changes:** `vt::can_fit` keeps a frame on the GPU only when its colors stay and it needs no padding. Other frames come down for the CPU.
  - The pixel transfer of the fitter can pad a frame (`kVTScalingMode_Letterbox`). Check its colors against the CPU's first.
  - A Metal compute pass on the IOSurface of the frame can pad it and convert its colors, as for the dissolves below.
- **Dissolves, burned-in subtitles, titles and placed pictures:** the frames come down while one of them shows.
  - [gpu-dissolves.md](gpu-dissolves.md), [gpu-burn-in.md](gpu-burn-in.md) and [gpu-placement.md](gpu-placement.md) plan a Metal pass for each.
- **ProRes 4444 with alpha in the viewer:** VideoToolbox gives it as AYUV64, its samples packed in one plane. The shader of the viewer doesn't read this layout, so it comes down.
