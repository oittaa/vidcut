# More compression for TIFF and OpenEXR

**When:** after #931.

**Why:** the Deliver page has these, and vidcut does not ([spec §10](../cut-page-spec.md#10-quick-export-ch-39-pp-788790-40954096-41044109), measured in Resolve Studio 21.2):

- OpenEXR, RGB half and RGB float: PIZ, DWAA, DWAB, HTJ2K 32 lines and HTJ2K 256 lines. DWAA and DWAB have a Compression Level (p. 4115).
- TIFF: the Compression Level of ZIP.

**What stops it:**

- FFmpeg 9.0.1's `exr` encoder has only `none`, `rle`, `zip1` and `zip16`.
- FFmpeg 9.0.1's `tiff` encoder has no option for the level of `deflate`. Its options are `dpi` and `compression_algo`.

**How:**

- The Rust `exr` crate (BSD-3-Clause) encodes PIZ.
- Its changelog says that 1.74.1 decodes DWAA and DWAB, and 1.74.2 encodes them. The DWAA and DWAB encoder takes a level.
- The `exr` crate has no HTJ2K.
- A TIFF with a ZIP level needs a deflate of vidcut's own, or a change in FFmpeg.
