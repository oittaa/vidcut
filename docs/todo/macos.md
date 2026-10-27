# macOS

**When:** the parts of Pictures on the GPU are next, one at a time. The other items come after the interchange and the smaller items ([roadmap](../roadmap.md)).

- `scripts/check.sh full` on a Mac.
- ProRes proxies on VideoToolbox checked on a Mac with ProRes hardware and on one without.
- An `.app` bundle with FFmpeg's libraries in it, signed.

## Pictures on the GPU

On macOS, VideoToolbox frames go to the viewer as Metal textures, and to the H.264, HEVC and ProRes encoders of VideoToolbox. A frame that comes down is read in place. These parts still go through the CPU:

- **The AI processes of macOS:** they keep VideoToolbox frames on the GPU, in and out, turned frames too. Frames that the CPU decodes go through RGB.
- **Placed pictures:** the frames of the picture come down to be placed, as on NVIDIA. The frame below stays on the GPU ([gpu-placement.md](gpu-placement.md)).
