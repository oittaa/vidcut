# Camera RAW

**When:** not decided.

A camera RAW file holds the values of the sensor, before the camera makes a picture of them. vidcut has no Camera Raw stage.

**Why:**

- A timeline that goes to Resolve for the grade often starts from RAW files ([roadmap](../roadmap.md#use-cases)). vidcut must read them to cut them.
- Camera Raw settings are the first stage of the picture ([spec §13](../cut-page-spec.md#13-order-of-operations-pp-287296-3407)).
- Kdenlive and Shotcut list RAW on their roadmaps ([Kdenlive](https://kdenlive.org/roadmap/), [Shotcut](https://shotcutapp.com/roadmap/)).

**In the code:**

- vidcut decodes with the FFmpeg libraries, and OpenEXR with the `exr` crate ([gpu](../design/gpu.md#the-gpu-and-the-decoder)).
- The design lists Camera Raw among the stages vidcut does not have ([picture order](../design/README.md#picture-order)).
- No code of vidcut names a RAW format. `tests/media-manifest.toml` has no RAW clip.
- FFmpeg 9.0.2 has decoders for Apple ProRes RAW and Cintel RAW.
- FFmpeg 9.0.2 has demuxers for REDCODE R3D, Magic Lantern Video and Phantom Cine.
- FFmpeg 9.0.2 has no decoder for Blackmagic RAW or ARRIRAW.
- FFmpeg 9.0.1 decodes ProRes RAW with compute shaders ([vulkan-video.md](vulkan-video.md)).

**To do:**

- Add a RAW clip of each format that FFmpeg reads to `tests/media-manifest.toml`.
- Check CinemaDNG. FFmpeg has a TIFF decoder. Whether it reads the pictures of a CinemaDNG folder is not checked.
- Check what vidcut does with each clip now: the probe, the picture, its colors and its proxy.
- Measure the Camera Raw settings of Resolve for each format: the decode quality, the white balance, the color space and the gamma.
- Measure what an OpenTimelineIO file keeps of those settings.
- Decide Blackmagic RAW. It needs the SDK of Blackmagic Design. Check its license against AGENTS.md rule 6.
- Proxies first: the cut plays from a proxy, and Resolve reads the RAW file again for the grade.
- The CLI, IPC and MCP perform the same action as the GUI.
