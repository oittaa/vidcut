# 4:2:2 and 10-bit planar pictures in the viewer

**In progress:** PR A (planar layouts), then PR B (VideoToolbox's P210).

The viewer takes 8-bit 4:2:0, NV12 and P010 only. So the CPU resamples ProRes, DNxHR and 10-bit planar pictures before the upload, and VideoToolbox's ProRes frames come down to be shown.

| `Layout` | Pixel formats | Given by |
|---|---|---|
| `Yuv420p10` | yuv420p10le | the CPU's HEVC, AV1 and VP9 at 10 bits |
| `Yuv422p` | yuv422p, yuvj422p | the CPU's DNxHR LB, SQ and HQ (vidcut's DNxHR proxies), MJPEG and H.264 4:2:2 |
| `Yuv422p10` | yuv422p10le | the CPU's ProRes 422, DNxHR HQX, H.264 and HEVC 4:2:2 at 10 bits |
| `P210` | p210le | VideoToolbox's ProRes 422 |

- Planar layouts go up a plane each. Their 10-bit samples are in the low bits of 16, P210's in the high bits, as P010's are.
- VideoToolbox gives ProRes 422 as `kCVPixelFormatType_422YpCbCr10BiPlanarVideoRange`, which FFmpeg 9.0 calls `AV_PIX_FMT_P210` (videotoolbox.c `videotoolbox_best_pixel_format`). Its IOSurface has two planes: Y at W×H and CbCr at W/2×H.
- The shader reads chroma as it does for 4:2:0, with the layout's luma rows per chroma row: 2 for 4:2:0, 1 for 4:2:2.

**Left out:**
- NVDEC's P210 (GPUs that decode 4:2:2, nvdec.c) comes down to be shown. No GPU here decodes 4:2:2 to test a copy on the GPU.
- NV16 (8-bit 4:2:2 interleaved, from VideoToolbox and NVDEC) comes down whole. Nothing here gives it to test.
- 12-bit 4:2:2, 4:4:4 and packed 4:2:2 (VA-API's and Direct3D 11's YUYV422 and Y210) are resampled to 4:2:0.
- Light measurements read 4:2:0 only, so their `Relayout` resamples the new layouts too.
- The backward buffer (`VideoFrames::reversed_frames`) counts 3 bytes a pixel. A 10-bit 4:2:2 picture takes 4.

**Checks:** PR A: `crates/vidcut/tests/picture.rs` draws ProRes, DNxHR and 10-bit HEVC pictures within a level of `Picture::to_rgba` where they are flat. PR B: on the M4 a ProRes clip plays with `VIDCUT_DEBUG=decoding` saying `Kept`, and `player state` showing a `P210` picture in `VideoToolbox memory`, `shown from GPU memory`.
