# GPU decoding for proxies and exports

**When:** later.

vidcut's own decoding (`vidcut-av`) runs on the CPU; only the preview player decodes on the GPU. Measured on 2026-10-11 with a 4K60 HEVC file: the CPU decodes it at about 240 fps, and the whole proxy encode ran at about 190 fps on every core. NVDEC (or VAAPI) decoding with scaling on the GPU would leave the CPU nearly idle and probably be faster. Frames must stay exact: the same pictures and timestamps as the software decoder, checked with the barcode fixtures.

It doesn't help image sequences: GPUs don't decode PNG, and decoding a 16-bit 4K PNG sequence (51 fps) was most of its proxy's time (41 fps).
