# Camera RAW

**When:** not decided.

A camera RAW file holds the values of the sensor, before the camera makes a picture of them. vidcut has no Camera Raw stage.

**Why:**

- The Cut page Inspector has the Camera Raw settings of a clip (p. 749).
- Kdenlive and Shotcut list RAW on their roadmaps ([Kdenlive](https://kdenlive.org/roadmap/), [Shotcut](https://shotcutapp.com/roadmap/)).

**What the manual says:**

- Raw decoding is the first image processing operation (p. 166, [spec §13](../cut-page-spec.md#13-order-of-operations-pp-287296-3407)).
- Chapter 7 gives the settings of each format (pp. 165–190).
  - The formats are ARRI, Blackmagic RAW, Canon RAW, CinemaDNG, Nikon RAW, Panasonic Varicam RAW, Phantom Cine, RED and Sony RAW.
- The settings are in three places (p. 165):
  - the Camera Raw panel of the Project Settings;
  - the Inspector of the Media, Cut and Edit pages;
  - the Camera Raw palette of the Color page.
- The Image panel of the Cut page Inspector has them (p. 749).
- Blackmagic RAW has Decode Quality, Decode Using, Color Science, White Balance, Color Space and Gamma (pp. 170–171).
  - It also has Exposure, Color Temp and Tint (p. 173).
- A search of the manual finds no ProRes RAW.

**In the code:**

- vidcut decodes with the FFmpeg libraries, and OpenEXR with the `exr` crate ([gpu](../design/gpu.md#the-gpu-and-the-decoder)).
- The design lists Camera Raw among the stages vidcut does not have ([picture order](../design/README.md#picture-order)).
- No code of vidcut names a RAW format. `tests/media-manifest.toml` has no RAW clip.
- FFmpeg 9.0.2 has decoders for Apple ProRes RAW and Cintel RAW.
- FFmpeg 9.0.2 has demuxers for REDCODE R3D, Magic Lantern Video and Phantom Cine.
- FFmpeg 9.0.2 has no decoder for Blackmagic RAW or ARRIRAW.
- FFmpeg 9.0.1 decodes ProRes RAW with compute shaders ([vulkan-video.md](vulkan-video.md)).

**To do:**

- Measure the lines of the manual above in Resolve. The manual and the application can differ.
- Add a RAW clip of each format that FFmpeg reads to `tests/media-manifest.toml`.
- Check CinemaDNG. FFmpeg has a TIFF decoder. Whether it reads the pictures of a CinemaDNG folder is not checked.
- Check what vidcut does with each clip now: the probe, the picture, its colors and its proxy.
- Measure what an OpenTimelineIO file keeps of the Camera Raw settings.
- Decide Blackmagic RAW. Find a library that decodes it, and check its license against AGENTS.md rule 6.
- The CLI, IPC and MCP perform the same action as the GUI.
