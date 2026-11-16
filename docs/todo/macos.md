# macOS

**When:** after the interchange and the smaller items ([roadmap](../roadmap.md)).

To do:

- Test ProRes proxies on a Mac with ProRes hardware and on a Mac without it.
- Make a signed `.app` bundle that includes FFmpeg's libraries.

## Workgroup memory on Metal

**Why:** on Metal, threads that write different components of one vector in workgroup memory can give wrong results ([gotchas](../gotchas.md#wgpu-and-egui)).

To do:

- NanoVSR (`upscale/nanovsr.rs`) writes single components of `raw` and `sm`. Make each thread write whole vectors.
- The FlashVSR matrix kernel (`upscale/flashvsr/gemm.rs`) writes single components of `sa` and `sb`. Make each thread write whole vectors.
- Check each upscaler on a Mac against a reference, as `scripts/face-check.py` does for the slow face model.

## Work on the CPU

Most frames stay on the GPU from the decoder to the viewer or the encoder. These cases use the CPU:

- **VideoToolbox exports without GPU decoding.** The encoder then has no frames of its own device. Thus the planes come back to memory.

To check by hand on the Mac:

- An export to VideoToolbox draws into the IOSurfaces of the encoder (`fs_encode_unorm`). Compare its frames with an export to the CPU encoder.
- The export's report has no `through_memory`.
