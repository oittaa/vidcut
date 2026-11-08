# Float pictures stay floats

**When:** not decided.

**Why:** an imported float picture loses its values above 1 and below 0.

- An OpenEXR is often scene-linear. Its values go above 1 and below 0.
- `openexr::Planes` gives float frames: `gbrpf16le`, `gbrpf32le`, `grayf16le`, `yaf16le` and the same formats with 32 bits.
- `Relayout` (`crates/vidcut-av/src/export/normalize.rs`) converts `gbrpf16le` and `gbrpf32le` frames to `gbrp16le`. It converts gray float frames to 12-bit YUV.
- These formats keep only the values from 0 to 1. The values above 1 and below 0 are lost before the draw.
- The draw mixes in 32-bit floats (`MIXING`). Thus the range of the mix is not used for these pictures.
- The stream of an OpenEXR has the transfer `linear`. vidcut reads the transfer only to find PQ and HLG (`hdr_of_transfer`).
- Thus a linear picture shows its values as they are. A flat 0.18 renders as 46 of 255.

**How:**

- Let float frames pass `Relayout`.
- Upload their planes to the GPU as float textures, and read them in `crates/vidcut-av/src/screen/picture.wgsl`.
- Check an export to a float OpenEXR: a value of 4.0 in the source must stay 4.0.
