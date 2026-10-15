# ProRes on NVIDIA's GPU through Vulkan

**When:** next, NVIDIA first ([gpu-frames.md](gpu-frames.md)).

**Where it is:** ProRes decodes on NVIDIA's GPU through FFmpeg's Vulkan decoder, the viewer shows it from there (preview.md §3), and exports and proxies take its frames on the GPU into NVENC and Vulkan's ProRes encoder (export.md §5, preview.md §6). What is open: automatic's choice, and the items under Later.

## Steps

Each PR lands something to see on the RTX 4080 SUPER, and is measured against the one before there: frames copied each way (`copies`), then CPU time, then time.
1. **Automatic prefers ProRes Proxy** (the user's decision): `AUTOMATIC` in proxy.rs becomes DNxHR LB, ProRes Proxy, AV1, HEVC, H.264. Where `prores_ks_vulkan` takes the decoder's frames on the GPU (on NVIDIA), it counts as made on the GPU (`round`: 0 where the GPU plays ProRes). Where Vulkan's ProRes isn't allowed it stays played only, as now. The docs' reason for AV1 and HEVC first, that ProRes's 4:2:2 frames come down to be shown, no longer holds on the Mac (P210) or on NVIDIA: D41 (cut-page-spec.md), preview.md §6, the tooltip in gui/preferences.rs and cli.md change with it. Tests in proxy.rs: `automatic_takes_apples_hevc_hardware` becomes ProRes on VideoToolbox. A new one: NVIDIA with `prores_ks_vulkan`, playing ProRes, takes `prores_ks_vulkan`. AMD's and the GPU-less cases stay.

**Later:**
- The encoder on a thread of its own. An export or a proxy decodes, makes each picture and encodes it on one thread, and the encoder holds that thread while the GPU encodes. A ProRes proxy of 10 s of 4K60 ProRes HQ takes 8.1 s on the RTX 4080 SUPER. `prores_ks_vulkan`'s `send_frame` takes 5.9 s of that, decoding 1.4 s, and libplacebo 0.8 s, of which its waits (`cuda::wait_done`) are 0.1 s. NVENC's `send_frame` takes 5.3 s of the clip's 9 s NVENC export at 4K. With the proxy's encoder on a thread of its own, fed through a channel of three pictures, the proxy took 6.8 s.
- Keeping the viewer's imports of Vulkan's frames across pictures. Each picture drawn is imported anew, which takes 2.4 ms of the viewer thread for a 4K picture on the RTX 4080 SUPER: 1.3 ms for the three planes' memory and 1 ms for the acquire's submission of its own, against 2 ms for uploading the picture. The exported handles already live as long as FFmpeg's frames context (`hw::vulkan`).
- Shrinking on Vulkan (`scale_vulkan`), and `bwdif_vulkan` for interlaced ProRes, which comes down before `bwdif` now.
- A 4:4:4 12-bit viewer layout (ProRes 4444).
- FFv1 on Vulkan: it has a barcode fixture (`VideoCodec::Ffv1`). FFmpeg 9.0.1 also decodes ProRes RAW, DPX and APV with compute shaders, and AV1, H.264, HEVC and VP9 with the GPU's video engine (Vulkan Video, [own-player.md](own-player.md)).
- Other vendors. A crash can't be caught, so a vendor's first test decode runs under a marker, as the encoders' tests do (`encoders.testing`): one left by a process that crashed keeps Vulkan's decoding off there.
