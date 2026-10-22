# More HDR metadata

**When:** later. What decision D46 ([design/export.md](../design/export.md#preparing)) leaves out.

- **Dolby Vision for pictures without it.**
  - vidcut writes Dolby Vision only when every picture has it.
  - Black before the first picture also makes vidcut leave Dolby Vision out.
  - Clips without Dolby Vision need an RPU made from their measured light (level 1: the minimum, average and peak in PQ).
  - Black at the start can take the RPU of the first picture.
- **HDR10+ that stops partway through an HEVC file.**
  - FFmpeg's HEVC decoder repeats the last HDR10+ SEI on every frame after it ([gotchas](../gotchas.md)).
  - Thus those frames keep a stale SEI. They do not get HDR10+ made from their light, as AV1 from libdav1d does.
  - To tell these frames apart, vidcut needs the packets that carry an HDR10+ SEI, by presentation time.
  - `hdr::insert_hdr10plus` writes such SEIs into packets. A new reader can find them in the packets.
  - Then remove the SEI from the decoded frames of the other packets.
- **Dolby Vision through the GPU's encoders** (NVENC, VAAPI, AMF, QSV).
  - FFmpeg makes the RPU only in libx265, libsvtav1 and libaom.
  - For the GPU's encoders, vidcut will put the RPU of each frame (`AV_FRAME_DATA_DOVI_RPU_BUFFER`) into the packets, as it does for HDR10+.
  - vidcut does not keep profiles 5 and 7 (no HDR10 base layer, or an enhancement layer).
- **Bindings.**
  - ffmpeg-sys-next does not bind `libavutil/mastering_display_metadata.h`.
  - Thus `hdr.rs` mirrors its structs (`AvMastering`, `AvLight`).
  - When ffmpeg-sys-next binds the header, remove the mirror.
