# ProRes on NVIDIA's GPU through Vulkan

**When:** next, NVIDIA first ([gpu-frames.md](gpu-frames.md)).

**Where it is:** ProRes decodes on NVIDIA's GPU through FFmpeg's Vulkan decoder, the viewer shows it from there (preview.md §3), exports and proxies take its frames on the GPU into NVENC and Vulkan's ProRes encoder (export.md §5, preview.md §6), and automatic proxies are ProRes Proxy there (spec D41). What is open is below.

## Open

- The encoder on a thread of its own. An export or a proxy decodes, makes each picture and encodes it on one thread, and the encoder holds that thread while the GPU encodes. A ProRes proxy of 10 s of 4K60 ProRes HQ takes 8.1 s on the RTX 4080 SUPER. `prores_ks_vulkan`'s `send_frame` takes 5.9 s of that, decoding 1.4 s, and libplacebo 0.8 s, of which its waits (`cuda::wait_done`) are 0.1 s. NVENC's `send_frame` takes 5.3 s of the clip's 9 s NVENC export at 4K. With the proxy's encoder on a thread of its own, fed through a channel of three pictures, the proxy took 6.8 s.
- A 4:4:4 12-bit viewer layout (ProRes 4444).
- FFv1 on Vulkan: it has a barcode fixture (`VideoCodec::Ffv1`). FFmpeg 9.0.1 also decodes ProRes RAW, DPX and APV with compute shaders, and AV1, H.264, HEVC and VP9 with the GPU's video engine (Vulkan Video, [own-player.md](own-player.md)).
- Other vendors. A crash can't be caught, so a vendor's first test decode runs under a marker, as the encoders' tests do (`encoders.testing`): one left by a process that crashed keeps Vulkan's decoding off there.
