# VideoToolbox's 4:2:2 in the viewer

**In progress:** PR B (VideoToolbox's P210).

ProRes frames VideoToolbox decodes come down to be shown: their surface, P210, isn't a layout the viewer takes.

- VideoToolbox gives ProRes 422 as `kCVPixelFormatType_422YpCbCr10BiPlanarVideoRange`, which FFmpeg 9.0 calls `AV_PIX_FMT_P210` (videotoolbox.c `videotoolbox_best_pixel_format`). Its IOSurface has two planes: Y at W×H and CbCr at W/2×H, 10 bits in the high bits of 16, as P010's.
- PR B: `Layout::P210`, `displayable` takes p210le, and the VideoToolbox import makes its planes R16Unorm and RG16Unorm textures.
- Checked on the M4: a ProRes clip plays with `VIDCUT_DEBUG=decoding` saying `Kept`, and `player state` showing a `P210` picture in `VideoToolbox memory`, `shown from GPU memory`.

**Left out:**
- NVDEC's P210 (GPUs that decode 4:2:2, nvdec.c) comes down to be shown. No GPU here decodes 4:2:2 to test a copy on the GPU.
- NV16 (8-bit 4:2:2 interleaved, from VideoToolbox and NVDEC) comes down whole. Nothing here gives it to test.
- 12-bit 4:2:2, 4:4:4 and packed 4:2:2 (VA-API's and Direct3D 11's YUYV422 and Y210) are resampled to 4:2:0.
- The backward buffer (`VideoFrames::reversed_frames`) counts 3 bytes a pixel. A 10-bit 4:2:2 picture takes 4.
