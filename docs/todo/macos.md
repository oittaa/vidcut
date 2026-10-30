# macOS

**When:** after the interchange and the smaller items ([roadmap](../roadmap.md)).

To do:

- Test ProRes proxies on a Mac with ProRes hardware and on a Mac without it.
- Make a signed `.app` bundle that includes FFmpeg's libraries.

## Work on the CPU

Most frames stay on the GPU from the decoder to the viewer or the encoder. These cases use the CPU:

- **Super Scale on frames other than SDR VideoToolbox frames.** HDR frames, tone-mapped frames and frames that the CPU decodes go through RGB in memory (`Scaler::scale`).
- **Retime Process on a placed clip.** The frames of its pair go through RGB in memory (`Input::Held`).
- **VideoToolbox exports without GPU decoding.** The encoder then has no frames of its own device. Thus the planes come back to memory.

To check by hand on the Mac:

- An export to VideoToolbox draws into the IOSurfaces of the encoder (`fs_encode_unorm`). Compare its frames with an export to the CPU encoder.
- The export's report has no `through_memory`.
