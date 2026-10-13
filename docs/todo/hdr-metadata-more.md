# More HDR metadata

**When:** later. What decision D46 (export.md §4, HDR metadata) leaves out.

- **Dolby Vision for pictures without it.** Dolby Vision is written only when every picture has it, and black before the first picture leaves it out. Clips without it would need an RPU made from their measured light (level 1: the minimum, average and peak in PQ); black at the start could take the first picture's.
- **HDR10+ that stops partway through an HEVC file.** FFmpeg's HEVC decoder puts the last HDR10+ SEI on every frame after it (gotchas), so those frames keep a stale one instead of HDR10+ made from their light, as AV1 decoded by libdav1d gets. Telling them apart needs the packets: which carry an HDR10+ SEI (`hdr::insert_hdr10plus` writes them; a reader would find them), by presentation time, and the decoded frames of the others stripped of it.
- **Dolby Vision through the GPU's encoders** (NVENC, VAAPI, AMF, QSV). FFmpeg makes the RPU only in libx265, libsvtav1 and libaom; for the GPU's encoders vidcut would put each frame's RPU (`AV_FRAME_DATA_DOVI_RPU_BUFFER`) into the packets, as it does HDR10+. Profiles 5 and 7 (no HDR10 base layer, or an enhancement layer) aren't kept.
- **Bindings.** ffmpeg-sys-next doesn't bind `libavutil/mastering_display_metadata.h`; `vidcut-av/src/hdr.rs` mirrors its structs. Binding it there would remove the mirror.
