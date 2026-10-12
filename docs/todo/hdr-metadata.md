# HDR mastering and light-level metadata

**When:** later.

HDR exports are tagged BT.2020 with PQ or HLG, but carry no mastering-display color volume (SMPTE ST 2086) or content light levels (MaxCLL, MaxFALL), even when the sources have them. Players and TVs then assume their own peak, so a PQ export can look brighter or dimmer than its source on some screens ([export.md §4](../decisions/export.md)).

Why it isn't simple:
- **Which values.** A timeline mixes clips: several HDR sources with different metadata, SDR clips placed at 203 nits, and HDR kept as it was. A sensible rule: the mastering display when every HDR source agrees, else none (or BT.2020 primaries at 1000 nits, the tone curve's peak); MaxCLL and MaxFALL as the highest of the sources, at least 203 for SDR material; HLG usually goes without.
- **Reading them.** They are in a stream's coded side data (MP4 `mdcv`/`clli`, Matroska) or in the frames' side data (HEVC and AV1 SEI/OBU). The probe would read them and the project keep them, like `hdr`.
- **Writing them.** Encoders take them differently (to check against FFmpeg 8.0's sources): libx265 and libsvtav1 read `AVCodecContext.decoded_side_data` (FFmpeg 7.0 and later), NVENC per-frame side data, and the muxers write container boxes from the stream's coded side data. The GPU path drops frame side data before libplacebo (it would tone-map by it), so it would add them back after.
- **Testing.** Fixtures with known metadata, read back from the output's stream and from its bitstream.
