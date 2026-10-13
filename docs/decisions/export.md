# Export

Tests: [`crates/vidcut-av/tests/export.rs`](../../crates/vidcut-av/tests/export.rs) and `hdr.rs` (GPU encoders with `--features vidcut-av/gpu-tests`), [`crates/vidcut/tests/cli.rs`](../../crates/vidcut/tests/cli.rs).

## 1. In-process and streaming

- Export runs in-process on the FFmpeg libraries (`vidcut-av::export`). `vidcut_core::render::flatten`, a pure function, plans it; the engine encodes the plan.
- **Plan:** video is a list of pieces (source ranges or black); audio is one or more lanes of pieces (source ranges or silence). Boundaries use exact integer math (`Rational::frame_to_sample`), so audio lengths never drift.
- **Engine:** for each output frame it encodes the next video frame, then exactly the audio samples that frame spans from each lane.
- **Memory stays flat** however many segments there are: each reader (video, and each audio lane) has only its current file open, and one helper thread per reader opens, seeks and decodes the next file while the previous piece encodes (`export/ahead.rs`).
- A piece in the same file as the previous one reads on, decoding forward when it starts at most 1 s ahead, else seeking.
- **Output** goes to a hidden `.<name>.part` file, renamed into place on success and deleted on errors and cancellation.
- **Damaged sources:** a packet the decoder rejects as invalid data is skipped, as players skip it: video shows the next frame, audio is silent there. Any other decoder error fails the export, as do more than 100 damaged frames in a row. Analysis decodes by the same rule (`decode::decode_stream`).

## 2. Sampling rule (preview and export)

Output frame `k` of a source piece shows the **first source frame whose timestamp is at least `origin + (source_frame + k)/fps − ½ frame`**.

- Sources at the project rate are frame-exact. At other rates it picks the nearest frame, ties going to the later one: a 25 fps clip on a 30 fps timeline shows source frames `0 1 2 3 3 4 5 6 7 8 8 9…`.
- The preview (the player engine, [preview.md](preview.md)) reads with the same readers, so it shows the export's frames at every rate.
- **Stills** are decoded once and shown on every frame of their pieces, never sought.
- **Image sequences** are read by FFmpeg's `image2` demuxer, picture `k` at timestamp `k` frames, so the rule picks pictures as it picks a video's frames. A missing file fails the export rather than starting later or repeating a picture.

## 3. Audio

- Each source piece is read sample-exactly: seek before the start, decode, and drop the samples before the start time.
- **Gaps follow the timestamps** (`decode::audio_gap`): a frame starting more than 2 ms after the previous one ended follows a gap (a packet missing or rejected), which becomes silence so later audio stays in sync. Audio that starts after its piece is silent until it begins.
- A frame stamped past the file's end (`decode::file_end`) has a broken timestamp and plays in order, with no gap; when the end is unknown, so does a jump of more than 1 s. Audio levels and waveforms count gaps the same way.
- libswresample converts to interleaved `f32` at the export's rate and layout. vidcut sizes its output buffers itself, because ffmpeg-next's wrapper can truncate when upsampling.
- Lanes are summed in Rust, with clip gain applied.
- **De-click:** 3 ms linear fades at every cut inside a lane, so silence-removal cuts don't click (`declick=false` turns them off).
- **Loudness normalization** (`audio.loudness`): [agents.md](agents.md) §5.

## 4. Video normalization

- Each source frame goes through a libavfilter graph: `scale` to fit inside the output keeping the display aspect, `pad` with black, centered, then the encoder's pixel format. The fit is computed in Rust (`export::video::fit`).
- **Rotation** (spec D33): the display matrix's turn is applied first (`transpose` or flips), and the fit uses the upright aspect.
- **Interlaced sources** (spec D34): the reader runs `bwdif` in decode order, before the sampling rule picks frames, so it sees each frame's real neighbors. A seek restarts it.

**Color** (`color.rs`, spec D35): every picture is converted from its tags to the output's colors.
- **SDR into SDR:** `scale` converts the matrix and range as it scales, with `accurate_rnd+full_chroma_int` when it converts (`color::CONVERT`): its default flags round colors low, by a level or two at white.
- **HDR into SDR:** `zscale` converts to linear light with the reference white (203 nits) at 1.0 and to BT.709 primaries, `tonemap=mobius` (transition 0.8, peak 1000 nits) rolls off the highlights, and `zscale` applies BT.709's transfer. Mobius, because `tonemap=hable` maps the reference white to a dark gray.
- **Into HDR** (PQ or HLG): the same linear light with SDR white at 203 nits (BT.2408), then BT.2020 and the output's transfer.
- Where `zscale` converts, it also writes the YUV, and swscale only scales and interleaves: swscale's own conversion comes out brighter.
- Encoders tag what they write: BT.709, or BT.2020 with PQ or HLG.

**Which output** (spec D44) is decided in `vidcut_core::export::color` before encoding, from each media's transfer and pixel format in the project (the engine, given `auto`, probes the files) and what the encoders can write (`encode::abilities`).

**HDR metadata** (spec D46; `vidcut_av::hdr`, `vidcut_core::export::metadata`) is chosen per file, after the dynamic range, from the project's values (the engine, given `auto`, reads the files' first decoded frames).
- **Static values** go to the encoder before it opens (`AVCodecContext.decoded_side_data`), so MP4/MOV and Matroska get them whatever the encoder.
- libx265, libsvtav1 and NVENC (in FFmpeg built with SDK 13) write them into the bitstream, and every frame carries them for VAAPI. A keyframe packet still without them gets them from vidcut (`hdr::insert_static`).
- **Measured light** (`vidcut_av::light`) comes from the frames the sampling rule picks, converted as the export converts them. Results are kept per file and time span (in memory and `~/.cache/vidcut/analysis`), so a piece measures only what is missing.
- The session measures before choosing the metadata, with progress (`measuring`); a plan measures nothing. A piece that can't be measured is left out, and the result says so.
- **Dynamic metadata:** each output frame drops its source's HDR metadata and gets back only the dynamic kinds the file keeps, from the frame it shows (`VideoFrames::with_dynamic`).
- **HDR10+:** FFmpeg's HEVC and AV1 encoders don't write it, so vidcut puts it into their packets (`hdr::insert_hdr10plus`); libvpx-vp9 takes it from the frames. Frames without it get it made from their measured light (`hdr::Hdr10Plus`); black frames get zeros.
- **Dolby Vision:** libx265 and libsvtav1 make each frame's RPU from its metadata (profile 8 for HEVC, 10 for AV1). Every frame needs one, so a black frame takes the last shown frame's.

## 5. Encoders

**Settings and presets:** settings are plain data in `vidcut_core::export`. Built-in presets follow the Quick Export models (spec §10; the Quick Export row is deviation X3). User presets are TOML files in `~/.config/vidcut/presets/`; `--set key=value` overrides any field.

**Choosing the encoder:**
- `auto` tries NVENC, then the other GPUs' own encoders (AMF and QSV on Windows, VAAPI on Linux), then the CPU's (`encode::AUTO_GPU`), and takes the first that opens: one that passed detection can still fail, as NVENC allows only a few sessions.
- On Linux, QSV is used only when named (it needs Intel's VPL runtime, and VAAPI serves the same GPUs), and AMF not at all (it needs AMD's closed driver).
- An encoder asked for by name fails the export if it won't open.
- **Detection** test-encodes every GPU encoder × pixel format × GPU and caches the result in `~/.cache/vidcut/encoders.json`, keyed by the FFmpeg versions, the system, the GPUs and their drivers (`encode::DETECTS`).
- AV1 quality is on the 0–63 scale of SVT-AV1's CRF and NVENC's CQ; the other encoders map it to their own (`encode.rs`).
- AMF's H.264 can write only the matrix, leaving the primaries and transfer unspecified; players take such HD as BT.709.
- With `gpu.acceleration` off, `export.plan` and `export.start` set every video encoder choice to the CPU (`export::as_preferred`).

**Frames on the GPU (NVENC)** (`cuda::CudaFrames`): a piece NVDEC decodes stays on the GPU from decoder to encoder. Other pieces, stills, sequences and black are made on the CPU (§4) and uploaded. `gpu_frames` counts the frames made on the GPU.
- `VideoReader::open_cuda` uses FFmpeg's own decoder, since for AV1 FFmpeg would pick libdav1d, which decodes on the CPU only.
- `scale_cuda` scales, after halving in bilinear steps so a big shrink doesn't alias. `bwdif_cuda` deinterlaces in decode order, as the CPU does (§4). `pad_cuda` places the picture on black.
- The GPU filters are built when the source opens; if they won't build, the piece uses the CPU.
- Each export and each piece works on its own CUDA stream (`CudaDevice::with_own_stream`), never the context's default stream, where waits for Vulkan block everyone.

**libplacebo on Vulkan** (`cuda::CudaPictures`) first does what the CUDA filters can't: turning (spec D33), other matrices and full range, placing 10-bit pictures, and HDR conversions.
- HDR uses the CPU's math (§4) in a user shader (`color::gpu_color`), because libplacebo's own tone mapping lifts black and maps on intensity (IPT) rather than the brightest channel.
- The Vulkan device is derived from the CUDA device once and shared, never closed, by every export and proxy (`CudaDevice::open`). Frames are copied CUDA → Vulkan → CUDA, never leaving the GPU.
- libplacebo's quirks (a filler frame after each frame, P010 as P016, no dither, side data dropped, never the same frame twice) are explained in `cuda.rs` and [gotchas.md](../gotchas.md).

**Decoded on the GPU, made on the CPU** (`export::run_decoding_on`): with `gpu.acceleration` and `gpu.decoding` on, pieces decode on the GPU the viewer decodes on (`GpuDecoder::best`), whatever the encoder, on a CUDA stream or device of their own (`GpuDecoder::own`).
- Frames that don't stay on the GPU come down to the CPU, shrunk on the GPU first when shown smaller (`hw::Shrinker`, [preview.md](preview.md) §6). `gpu_decoded` counts the frames decoded on the GPU.
- `gpu.decoding` off decodes every piece on the CPU and the GPU only encodes, which is faster where the GPU is small ([todo](../todo/gpu-decoding.md)).

**Frames on the GPU (VAAPI, AMF):** an encoder on the GPU the pieces decode on opens on the decoder's device (AMF on frames made there, as it takes textures only from the frames it was opened with), and pieces that need only turning and fitting stay on the GPU. QSV takes frames from memory.
- VAAPI turns, scales and pads with `transpose_vaapi`, `scale_vaapi` and `pad_vaapi` (`hw::Fitter`); AMF's video processor draws them turned, scaled and placed on black (`hw::d3d11::Scaler::into`), and a fit that fails leaves that file to the CPU.
- Pieces in other colors than the output's (`color::kept`), interlaced ones and black are made on the CPU and uploaded, as are 8-bit pieces into 10-bit AMF output, which AMD's video processor won't draw.

**Passes** (spec D45, which lists the settings and defaults):
- **Two runs** (`Passes::TwoRuns`; x264, x265, VP9): the first run encodes every frame with `AV_CODEC_FLAG_PASS1`, muxing nothing; the second opens the encoder again with `PASS2` and the statistics. **Two inside one encode** (`Passes::TwoInside`): NVENC's `multipass`.
- The others make one pass; SVT-AV1 too, as FFmpeg 8.0's wrapper ignores the pass flags ([todo](../todo/svt-av1-two-passes.md)). Two passes asked of them is an error for that encoder, so `auto` moves on.
- The statistics go in a hidden `.<name>.passes` folder beside the output, removed however the export ends: x264's and x265's grow too large at 4K for a RAM-backed `/tmp`.
- Loudness normalization measures the mix before the second run; it decodes only audio, so it isn't folded into the first.
- `crates/vidcut-av/examples/passes.rs` compares pass settings by bitrate and PSNR.

## 6. Quick Export semantics

- With no range, an export covers the In–Out when both marks are set, else the whole timeline (spec §10). With no preset, the extension picks `h264-master`'s settings for video containers, or the matching `audio-*` preset for audio extensions.
- **Per-clip export** writes one file per clip in the range, without the neighbors' J/L audio, named by a template (`{index:03}-{name}`).
- Existing files are kept unless `overwrite` is set.

## 7. Logging

FFmpeg's own logging defaults to fatal only: decoders recovering after a seek log harmless errors, and real failures surface as vidcut errors with context. `VIDCUT_AV_LOG` ([tools.md](../tools.md)) shows more.
