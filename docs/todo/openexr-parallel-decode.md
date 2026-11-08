# One OpenEXR frame on many threads

**When:** not decided.

**Why:** a seek or a still waits for one OpenEXR frame, and one frame decodes on one thread.

- `openexr::Planes` decompresses the blocks of a frame one after the other.
- On a 16-core CPU, a 4K float PIZ frame takes about 1 s. A 4K half PIZ frame takes about 180 ms.
- The same frames decompress in about 120 ms and 30 ms with all cores.
- An image sequence that plays decodes many frames at the same time, one frame on each thread of the pool. Thus playback is not slower.

**How:**

- Read the compressed chunks of the frame in sequence. Decompress them as slices with `pool::run`, and write their lines into the planes.
- Do not use `decompress_parallel` of the `exr` crate. It makes a new rayon thread pool for each frame.
- When the pool already decodes one frame on each thread, do not divide a frame into slices.
