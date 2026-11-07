# More compression for TIFF and OpenEXR

**When:** not decided.

**Why:** the Deliver page has these, and vidcut does not ([spec §10](../cut-page-spec.md#10-quick-export-ch-39-pp-788790-40954096-41044109), measured in Resolve Studio 21.2):

- OpenEXR: the Compression Level of ZIP and ZIP1, from 0 to 9. The default is 4.
- OpenEXR: HTJ2K 32 lines and HTJ2K 256 lines.
- TIFF: the Compression Level of ZIP, from 0 to 9. The default is 5.

**What stops it:**

- The `exr` crate (1.74.2) compresses ZIP and ZIP1 at the fixed zlib level 4. It has no setting for the level.
- The `exr` crate has no HTJ2K.
- FFmpeg 9.0.1's `tiff` encoder has no option for the level of `deflate`. It uses zlib's default level, 6.

**How:**

- ZIP levels of OpenEXR: a setting for the level in the `exr` crate, sent to its project.
- HTJ2K: an encoder of High-Throughput JPEG 2000, for example OpenJPH (BSD-2-Clause, C++).
- The ZIP level of TIFF: a deflate of vidcut's own (`flate2` is already a dependency), or a change in FFmpeg.
- Then `video.compression_level` also takes these levels.
