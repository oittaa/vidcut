# More HDR metadata

**When:** later. What decision D46 (export.md §4, HDR metadata) leaves out.

- **Measured light levels.** MaxCLL and MaxFALL come from the sources, or are left out when a PQ source has none. A pass over the output's pictures (as Resolve's Light Level report measures, p. 282) would give the real ones, also for timelines that mix sources.
- **Dynamic metadata for mixed timelines.** HDR10+ and Dolby Vision are kept only when every picture has them, and a gap's black frames leave Dolby Vision out (an encoder needs an RPU on every frame). Frames without (SDR clips, PQ clips without them, titles) would need some made for them: HDR10+'s statistics, Dolby Vision's level 1, measured from the pictures; black frames could take a neighbor's RPU (black stays black under any trim).
- **Dolby Vision through NVENC and VAAPI.** FFmpeg makes the RPU only in libx265, libsvtav1 and libaom; for the GPU's encoders vidcut would put each frame's RPU (`AV_FRAME_DATA_DOVI_RPU_BUFFER`) into the packets, as it does HDR10+. Profiles 5 and 7 (no HDR10 base layer, or an enhancement layer) aren't kept.
- **Bindings.** ffmpeg-sys-next doesn't bind `libavutil/mastering_display_metadata.h`; `vidcut-av/src/hdr.rs` mirrors its structs. Binding it there would remove the mirror.
