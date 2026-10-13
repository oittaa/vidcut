# Export

Tests: [`crates/vidcut-av/tests/export.rs`](../../crates/vidcut-av/tests/export.rs) and `hdr.rs` (GPU encoders with `--features vidcut-av/gpu-tests`), [`crates/vidcut/tests/cli.rs`](../../crates/vidcut/tests/cli.rs).

## 1. In-process and streaming

- Export runs in-process on the FFmpeg libraries (`vidcut-av::export`), with no ffmpeg subprocesses.
- `vidcut_core::render::flatten`, a pure function, plans the work. The engine encodes the plan.

**Plan:**
- Video is a list of pieces: source ranges or black.
- Audio is one or more lanes of pieces: source ranges or silence.
- Boundaries use exact integer math. Audio boundaries use `Rational::frame_to_sample` (i128, rounded), so lengths never drift: at 29.97 fps and 48 kHz, frames alternate between 1601 and 1602 samples.

**Engine:**
- For each output frame, it takes the next video frame from the current piece's decoder, then exactly the audio samples that frame spans from each lane, and encodes them.
- Memory stays flat however many segments there are. Each reader (video, and each audio lane) has only its current file open, plus the next one while that opens. A 400-segment test checks this in a process of its own.
- A two-pass export (§5, Passes) runs the video through the encoder twice: once to measure, muxing nothing, then as above.

**Starting a piece** (`export/video.rs`, `export/audio.rs`, `export/ahead.rs`):
- **Same file as the previous piece:** the reader reads on. If the piece starts at most 1 s past the decoder's position, it decodes forward to the first frame (audio skips samples). Otherwise it seeks in the open file. A frame repeated across the cut needs neither.
- **Another file:** a helper thread opens it, seeks, and decodes up to its first frame while the previous piece encodes. The piece then takes the opened source. If opening failed, the piece fails with that error.
- Each reader has one opener thread for the whole export, not one per piece: glibc gives every new thread its own malloc arena, and decoder memory freed across many arenas stays resident.

**Output:** written to a hidden `.<name>.part` file and renamed into place on success. Errors and cancellation delete it.

**Damaged sources:** a packet the decoder rejects as invalid data is skipped, as players skip it, and the export goes on.
- Video shows the next frame in its place (the sampling rule, §2). Frames that refer to it show whatever the decoder makes of them, up to the next keyframe.
- Audio is silent there (§3).
- Any other decoder error fails the export. So do more than 100 damaged frames in a row (`decode::DAMAGED_IN_A_ROW`).
- Audio levels, scene measurements and waveforms decode by the same rule (`decode::decode_stream`).

## 2. Sampling rule (preview and export)

Output frame `k` of a source piece shows the **first source frame whose timestamp is at least `origin + (source_frame + k)/fps − ½ frame`**.

- Sources at the project rate are frame-exact.
- At other rates it picks the nearest frame, and ties go to the later frame. A 25 fps clip on a 30 fps timeline shows source frames `0 1 2 3 3 4 5 6 7 8 8 9…`.
- The preview (the player engine, [preview.md](preview.md)) reads with the same readers, so it shows the export's frames at every rate.

**Stills and image sequences:**
- A still's picture is decoded once and shown on every frame of its pieces. It is never sought: an image file has nothing past its picture.
- A sequence is read by FFmpeg's `image2` demuxer (`decode::Input::Sequence`). Picture `k` has timestamp `k` in a time base of one frame at the sequence's rate, so the rule above picks pictures as it picks a video file's frames.
- The run is the contiguous files from the first number. image2's `start_number_range` is 1, so a missing first file fails the open instead of starting at a later number.
- A run that ends before the sequence's count (a missing file) fails the export instead of repeating the picture before the hole.
- The last picture lasts one frame at the sequence's rate, like a video's last frame. At a project rate above the sequence's, the last output frame or two sample a time just past it.

## 3. Audio

**Reading:**
- Each source piece is read sample-exactly: seek before the start, decode, find the first frame's timestamp, and drop the samples before the start time.
- Decoders get the packet time base (`pkt_timebase`). FFmpeg needs it to shift timestamps past skipped priming samples (Opus pre-skip, AAC priming).

**Gaps follow the timestamps** (`decode::audio_gap`):
- A frame that starts more than 2 ms after the previous one ended follows a gap: a packet the file lacks, or one the decoder rejected. Smaller differences are timestamp rounding (Matroska keeps milliseconds).
- A gap becomes silence, so later audio stays in sync with the picture. The silence is made a second at a time as it is read, so a big jump in the timestamps costs no memory.
- Audio that starts after the piece starts (a stream that begins after the picture, or a lost first packet) is silent until it begins.
- A frame stamped past the file's end has a broken timestamp. It plays in order, with no gap, so one bad timestamp doesn't silence the rest of the piece.
- The file's end (`decode::file_end`) is the container's start plus its duration. Without a container start, the duration is added to the audio stream's start, so timestamps on an offset clock don't look past the end. With no start at all, or no positive duration, the end is unknown.
- When the end is unknown, a jump of more than 1 s plays in order too. A shorter one is still silence.
- Audio levels and waveforms count gaps the same way, so silence a measurement finds is where the export has it.

**Conversion and mixing:**
- libswresample converts to interleaved `f32` at the export's rate and layout: mono is upmixed, 5.1 downmixed. vidcut sizes the output buffers itself, because ffmpeg-next's wrapper can truncate when upsampling.
- Lanes are summed in Rust, with clip gain applied.
- **De-click:** 3 ms linear fades at every cut inside a lane, so silence-removal cuts don't click. On by default; `declick=false` turns them off.
- **Loudness normalization** (`audio.loudness`): [agents.md](agents.md) §5.

## 4. Video normalization

- Each source frame goes through a libavfilter graph: `scale` (fit inside the output, keeping the display aspect and honoring the sample aspect ratio), `pad` (black, centered), `setsar=1`, `format=<encoder pixel format>`.
- The fit is computed in Rust (`export::video::fit`).

**Rotation** (spec D33): the stream's display matrix turns the picture upright first, with `transpose=clock` or `cclock`, or `hflip,vflip` for 180°. The fit then uses the upright aspect. Thumbnails turn their pictures the same way.

**Interlaced sources** (spec D34):
- `VideoReader` deinterlaces streams flagged interlaced with `bwdif=mode=send_frame:deint=interlaced`: one frame out per frame in, the first field kept, progressive frames passed through. NVDEC's frames go through `bwdif_cuda` with the same options (§5).
- It runs in decode order, before the sampling rule picks frames, so `bwdif` sees each frame's real neighbors.
- A seek restarts it, and the frames decoded on the way to the target feed it. `bwdif` halves its output time base, so timestamps are rescaled back to the stream's.
- Thumbnails aren't deinterlaced: at pool size, combing barely shows.

**Color** (`color.rs`, spec D35): every picture is converted to the output's colors. A picture's colors come from its tags. Untagged pictures get what players assume: BT.709 when 1280 wide or over 576 lines, BT.601 otherwise, and BT.2020 for HDR.
- **SDR into SDR:** `scale` converts the matrix and range to BT.709 limited range as it scales.
  - When swscale converts colors (another matrix or range, YUV to RGB or back), it gets `accurate_rnd+full_chroma_int` (`color::CONVERT`): with its default flags it rounds colors low, enough to move white by a level or two. The flags cost time, so pictures it only resamples go without them.
- **HDR into SDR:** `zscale` converts to linear light with the reference white (203 nits) at 1.0, then to BT.709 primaries. `tonemap=mobius` (transition 0.8, peak 1000 nits) maps the result, and `zscale` applies BT.709's transfer.
  - Everything up to 0.8 of the reference white stays as it was, so SDR-graded material in an HDR file looks as it did. Highlights roll off above it. PQ's 203 nits and HLG's 75% both come out at about 95% white.
  - The curve is mobius because `tonemap=hable` maps the reference white to a dark gray.
- **Into HDR** (PQ or HLG): the same linear light, with SDR white at 203 nits (BT.2408), then BT.2020 and the output's transfer. SDR white lands at PQ code 573 of 1023.
- **Output tags:** encoders tag what they write: BT.709, or BT.2020 with PQ or HLG.
- **zimg:** `zscale` needs explicit output primaries when it converts to linear light. For untagged frames it otherwise finds no path.
- **swscale only scales what `zscale` converts:** `scale` is told the source's matrix and range for both input and output, so it leaves them alone. Told nothing, it would convert to BT.601, because the graph's source carries no colorspace. `zscale` writes the YUV itself, in planar form even for NVENC's NV12 and P010, which swscale then only interleaves: swscale's own RGB-to-YUV conversion comes out brighter than zimg's.

**Which output** (spec D44) is decided in `vidcut_core::export::color` before encoding:
- The session takes each media's transfer and pixel format from the project (`pixel_depth` reads FFmpeg's pixel format descriptor). The engine, given `auto` directly, probes the files.
- What the encoders can write comes from the encoder cache and the CPU encoders' pixel formats (`encode::abilities`).
- What the plan and result report: [cli.md](../cli.md).

**HDR metadata** (spec D46; `vidcut_av::hdr`, `vidcut_core::export::metadata`) is chosen per file, after the dynamic range. The sources' values come from the project, or from the files when the engine is given `auto` directly. The probe reads them from the first decoded frame, where FFmpeg puts the container's values unless the bitstream has its own.

*Static values:*
- They go to the encoder before it opens (`AVCodecContext.decoded_side_data`).
- libx265 writes them into the bitstream at every keyframe (`repeat-headers=1`), not only in `hvcC`. libsvtav1 writes them on every keyframe. NVENC writes them when FFmpeg was built with SDK 13 headers.
- FFmpeg's `ff_encode_preinit` copies them into the stream's parameters, so MP4/MOV (`mdcv`/`clli`) and Matroska/WebM write them whatever the encoder.
- Every frame carries them too, for VAAPI, which reads them only there.
- A keyframe packet still without them gets them from vidcut (`hdr::insert_static`): the mastering display and light level SEI, or AV1's HDR_MDCV and HDR_CLL metadata OBUs. This covers NVENC in FFmpeg builds without SDK 13, such as Ubuntu's FFmpeg 8.0.
- x265 also gets `hdr10-opt=1` for PQ.

*Measured light* (`vidcut_av::light`):
- Light is measured on the frames a piece shows, decoded on the CPU and read as the export converts them: PQ's EOTF; HLG's inverse OETF with 1.2 system gamma on a 1000 cd/m² display; SDR as BT.1886 with white at 203 cd/m², in BT.2020.
- Each frame gives each channel's peak over every pixel, so a one-pixel highlight counts.
- From a grid of at most half a million pixels (every pixel of smaller frames), each frame also gives the average of each pixel's brightest channel, and that channel's percentiles from a histogram with 0.1 cd/m² bins up to 100 cd/m² and 1 cd/m² bins above.
- A piece's MaxCLL and MaxFALL count only the frames the sampling rule (§2) picks (`light::sampled`, with `export::video::target`). At another rate, skipped frames don't count, and the last frame shown may lie past the span's end (measuring a span keeps the frame after it).
- Results are kept per file with the time spans they cover, keyed by path, size and time, in memory and in `~/.cache/vidcut/analysis`. A piece measures only the part of its span that isn't there yet.
- The session measures what an export needs before choosing its metadata (`export::choose_colors` with `Measuring`) and reports progress with `measuring` set. A plan measures nothing. The engine, run directly, measures what is missing without progress.
- A piece that can't be measured is left out, and the result says so (`No light levels: a picture couldn't be measured`).
- **Finding the peak cheaply:** the four pixels of a 2×2 block share one chroma sample, so each channel's code is the luma plus what the chroma adds, and its light rises with it (the transfers rise, and SDR's gamut into BT.2020 has no negative coefficient). So the block's brightest luma gives its brightest channels. A block row is skipped when its brightest luma, with its chroma's extremes, can't beat the peak found so far. In the other rows, each block whose luma can't beat it is skipped. HDR keeps each channel's highest code and looks up its light once.

*Dynamic metadata on each output frame:*
- An output frame drops its source's HDR metadata (the CPU's `scale` passes it on, and VAAPI and libvpx would write it).
- It gets back only the dynamic kinds the file keeps, from the frame it shows (`VideoFrames::with_dynamic`), after scaling, libplacebo or the upload, which drop or change them.
- The decoded frame's side data is set aside as it is decoded, because the GPU's scaler takes the frame itself (`av_buffersrc_add_frame` moves it).

*HDR10+:*
- FFmpeg's HEVC and AV1 encoders don't write it (libaom does in FFmpeg 9.0), so vidcut puts it into their packets (`hdr::insert_hdr10plus`): `av_dynamic_hdr_plus_to_t35` after the T.35 header Matroska also writes, in a prefix SEI before the first slice (escaped, at the slice's temporal id) or in a metadata OBU before the first frame OBU.
- libvpx-vp9 takes it from the frames, and WebM/Matroska store it as BlockAdditions.
- Frames without it get it made from their measured light (`hdr::Hdr10Plus`, attached with `av_dynamic_hdr_plus_from_t35`): one window, no target display, each channel's peak, the average, the percentiles at 1, 5, 10, 25, 50, 75, 90, 95 and 99%, and no curve.
- Files whose first frame has none are measured ahead. In other files, a frame without it is measured as it is decoded (`light::Meter`, before the GPU's scaler takes it; NVDEC's frames are copied down). So when HDR10+ stops partway through a file, the frames after it get HDR10+ made, where the decoder gives them none, as libdav1d does for AV1. FFmpeg's HEVC decoder repeats the last HDR10+ on them instead ([gotchas.md](../gotchas.md)), and they keep that.
- Black frames get zeros.

*Dolby Vision:*
- libx265 and libsvtav1 make an RPU from each frame's `AV_FRAME_DATA_DOVI_METADATA` when `dolbyvision=1`: profile 8 for HEVC (the base layer's compatibility from its tags), 10 for AV1.
- A frame without one is an error, so every picture must have it. A black frame takes the RPU of the last frame shown (`VideoFrames` keeps it).
- x265 needs a VBV for it: peaks of 1.5 times a target bitrate, or 160 Mb/s at constant quality.
- MP4 writes `dvcC` only with `strict=unofficial`.

*FFmpeg structs:* ffmpeg-sys-next doesn't bind `mastering_display_metadata.h`, so `hdr.rs` mirrors the two structs' fields and takes their sizes from FFmpeg's allocators (they may grow). HDR10+ and Dolby Vision stay FFmpeg's opaque buffers, copied by reference.

## 5. Encoders

**Settings and presets:**
- Settings are plain data in `vidcut_core::export`: container, video (codec, encoder, quality or bitrate, speed, profile, bit depth, size, dynamic range) and audio (codec, bitrate, rate, channels).
- The dynamic range and an unset bit depth are chosen per file (§4, spec D44). Presets leave them to that, except TikTok's, which asks for SDR.
- Built-in presets follow Resolve's Quick Export models (spec §10): H.264/H.265 Master, ProRes 422 HQ, YouTube 720p/1080p/2160p, Vimeo, TikTok. There are also AV1, VP9, DNxHR HQ and five audio-only presets. The Quick Export row shows H.265 Master, AV1, ProRes 422 HQ, YouTube 1080p and 2160p, and TikTok (deviation X3).
- User presets are TOML files in `~/.config/vidcut/presets/`. `--set key=value` overrides any field.

**Choosing the encoder:**
- `auto` tries NVENC, then the other GPUs' own encoders, then the CPU's (`encode::AUTO_GPU`). The other GPUs' encoders are AMF (AMD) and QSV (Intel) on Windows, and VAAPI (AMD and Intel) on Linux.
- On Linux, QSV is used only when asked for by name: it needs Intel's VPL GPU runtime (`libmfx-gen1.2` on Ubuntu), and VAAPI serves the same GPUs without it. vidcut doesn't use AMF on Linux, where it needs AMD's closed driver.
- An export takes the first of these that opens (`encode::video_encoders`). A GPU encoder that passed detection can still fail to open: NVENC allows only a few sessions at a time, and other exports may hold them. The report names the encoder used; the plan (`--dry-run`) names the first choice.
- An encoder asked for by name (`nvenc`, `vaapi`, `amf`, `qsv`, `cpu`) fails the export if it won't open. If none opens, the error says why each didn't.
- An explicit `video.speed` goes only to the encoder the export starts with. Encoders don't share speed names (x264's presets, NVENC's p1–p7, AMF's `speed` to `quality`, QSV's `veryfast` to `veryslow`, SVT-AV1's numbers), so an encoder the export falls back to uses its own default.
- **Detection** test-encodes every GPU encoder × pixel format × GPU: VAAPI on each render node; QSV on each Intel GPU (its render nodes on Linux, its adapters on Windows, passed as `child_device` because FFmpeg reads QSV's own device string as the implementation to load); AMF on each AMD adapter. The result is cached in `~/.cache/vidcut/encoders.json`, keyed by the FFmpeg versions, the system, the GPUs and their drivers, and what detection tests (`encode::DETECTS`).

**AMD's and Intel's encoders** take NV12 or P010 frames from memory, each on a device on its GPU: QSV on its own device (over a VA-API device on Linux), AMF on a Direct3D 11 device on its adapter. Where a decoder on the same GPU decodes the pieces, VAAPI and AMF take that decoder's frames on its device instead (below).
- **AMF:** a constant quantizer (`rc=cqp`, as VAAPI uses; AMF's quality-led `qvbr` counts the other way and stays under a bitrate), or a bitrate with peaks 1.5 times as high (`vbr_peak`). Preset `quality`. H.264 High (baseline is its `constrained_baseline`), HEVC Main or Main 10.
- AMF's H.264 can leave the primaries and transfer unspecified in the bitstream and write only the matrix (BT.709), though FFmpeg gives it all three. Players take such HD as BT.709.
- **QSV:** its intelligent constant quality (`global_quality`, 1–51 for every codec), or a bitrate with peaks 1.5 times as high. Preset `medium`. Profiles as AMF's.
- **AV1 quality** is on the 0–63 scale of SVT-AV1's CRF and NVENC's CQ. VAAPI and AMF take it as a quantizer index (0–255; VAAPI's in `global_quality`, since its AV1 has no `qp`). QSV scales it to 1–51.
- **HDR:** the static values come from each frame (`hdr::attach`), and their HEVC carries them at every keyframe and in the container. HDR10+ goes into their packets as for the other encoders (§4).
- FFmpeg silently drops an option an encoder doesn't have. The test `options_are_the_encoders_own` checks every option vidcut sets on every encoder this FFmpeg has.

**GPU switches:** all GPU work, the GPU encoders included, needs the preferences' `gpu.acceleration` on (the default). When it is off, `export.plan` and `export.start` set every video encoder choice to the CPU (`export::as_preferred`): a preset naming NVENC still exports, in software, and proxies use x264 or SVT-AV1.

GPU tests (`gpu-tests`) compare every frame with the CPU's export: `gpu_decoded_exports_match_the_cpu`, `vaapi_exports_keep_frames_on_the_gpu`, `amf_exports_keep_frames_on_the_gpu` and the NVENC tests in `tests/export.rs`.

**Frames on the GPU (NVENC):**
- NVENC opens on frames in GPU memory (`cuda::CudaFrames`: NV12, or P010 for 10-bit output).
- **A piece qualifies** when it is a video file NVDEC decodes. It decodes on NVDEC and scales with `scale_cuda` (`VideoFrames::with_gpu`). The sampling rule picks its frames as on the CPU, so other rates repeat or skip GPU frames the same way.
- `VideoReader::open_cuda` uses FFmpeg's own decoder, which can use NVDEC. For AV1, FFmpeg would otherwise pick libdav1d, which decodes on the CPU only.
- **Halving:** `scale_cuda` samples only a few pixels per output pixel, so a big shrink would alias. It first halves in bilinear steps (each output pixel the average of four), then scales with bicubic.
- **Interlaced** pieces are deinterlaced by the reader with `bwdif_cuda`, in decode order and before the sampling rule picks frames, as the CPU reader runs `bwdif` (§4). A deinterlacer after the picking would see gaps at other rates and after seeks.
- **Another aspect:** the picture is fitted and centered on black with `pad_cuda`, at even offsets, as FFmpeg's `pad` rounds them for 4:2:0. `pad_cuda` takes no 10-bit frames, so in 10-bit output libplacebo places the picture (below).
- The source builds its GPU filters and makes its first picture when it opens (on the thread that opens the next piece ahead). If the filters won't build, the piece uses the CPU.
- Other pieces, stills and sequences decode and normalize on the CPU (§4), and black frames are made there. They are uploaded into the encoder's frames.
- Software encoders and QSV take frames from the CPU. VAAPI and AMF take their decoder's frames on the GPU where they can (below).
- **CUDA streams:** each export, and each piece in it (the next piece opens ahead on another thread), works on its own CUDA stream (`CudaDevice::with_own_stream`). The encoder takes a frame once its stream's work is done. On the context's default stream, all work runs in one line: a wait for Vulkan there holds up everyone's decoding and encoding, and two users each waiting for Vulkan work queued behind the other's deadlock.
- **Black frames** (gaps, black pieces) in P010 keep their 10 bits in the high bits. Written low, they are a green-tinted black below black.
- The report counts the frames made on the GPU (`gpu_frames`).

**libplacebo on Vulkan** (`cuda::CudaPictures`): the CUDA filters can't turn pictures or change their colors, so these pictures go through libplacebo first, at the source's size:
- **Turned pictures** (spec D33, phone video): turned upright with libplacebo's `rotate` (clockwise, as the display matrix says).
- **Other matrices** (BT.601: SD, and VP8, which always signals BT.601; BT.2020 SDR) **and full range:** the input is tagged with the matrix and range the CPU reads (`color::gpu_color`; untagged SD as BT.601) and with the output's primaries and transfer, so only the matrix and range change, as when swscale converts them.
- **10-bit output with another aspect:** libplacebo also scales and places the picture, in the same pass. It can't clear a border (it clears with a copy that FFmpeg's Vulkan frames don't allow), so it gets two inputs: a black frame of the output's size, and the picture over it.
- **HDR** (HDR into SDR, and anything into PQ or HLG) uses the CPU's math (§4) in a libplacebo user shader (`color::gpu_color`): PQ's EOTF, or HLG's inverse OETF and 1.2 gamma, per channel (zimg's approximate path, which `zscale` takes); linear light with the reference white at 1.0; the gamut matrix; the mobius curve on the brightest channel; negative channels to black; then the output's transfer.
  - libplacebo is told the picture is already in the output's primaries and transfer, so it only reads the matrix, scales, places and writes.
  - The shader hooks the scaled picture, as the CPU scales before converting. libplacebo scales and places every HDR picture, since the CUDA filters would scale converted pictures.
  - HDR kept in the same transfer passes through the CUDA filters, as the CPU leaves it alone.
  - libplacebo's own tone mapping can't give the CPU's curve: it lifts black for an SDR display's contrast (BT.1886 with a black level, which no option sets), and it maps on intensity (IPT) rather than the brightest channel, leaving colors outside BT.709 out of range. Hence the user shader.
  - **Half floats:** libplacebo keeps a picture between passes in half floats, which the GPU stores rounded toward zero. The shader reads each value as the middle of its step and moves what it writes half a step up. Without that, the GPU's pictures come out darker than the CPU's.
- **Devices:** the Vulkan device is derived from the CUDA device once. Both are shared by every export and proxy for the whole process and never closed (`CudaDevice::open`), because the Vulkan loader crashes when two exports create Vulkan instances at the same time. Each frame is copied CUDA → Vulkan, converted, and copied back, never leaving the GPU (Vulkan → CUDA mapping isn't supported; copies are).

**libplacebo's quirks:**
- FFmpeg's filter, made without its Vulkan device, converts on the CPU in front of it. vidcut makes it by hand, with the device set before it starts.
- It gives out a frame's picture only once it has the next frame. An export needs each picture at once, so a filler frame follows each frame, and the filler's pictures are dropped. Proxies skip the filler, which halves the rendering.
- **Never give it the same Vulkan frame twice at once** (`cuda::Blacks`). libplacebo takes a frame at the semaphore value the frame says and gives it back at the next value. A frame held twice ends a step ahead of anything the GPU signals, and FFmpeg, freeing it, waits forever. So each black frame it gets (a filler, or the background under a placed picture) is one it doesn't already hold, and the pool refills as frames come back.
- Its frame queue takes a frame within a millisecond of the render time as that time's frame, so the timestamps it sees are counts a second apart.
- A Vulkan device derived from CUDA keeps each plane of a frame in its own image, and libplacebo misses P010's 6-bit shift in such planes. So it gets 10-bit frames as P016, which has the same layout with all 16 bits used: they copy unchanged, and NVENC reads the top 10 bits.
- Its blue-noise dither lifts black by most of a 10-bit level, so it rounds instead.
- A frame's side data is dropped before the frame goes in. The filter copies the frame's properties to its picture and libplacebo turns both by their display matrices, so the turns would cancel. It would also tone-map by HDR metadata and read ICC profiles, which the CPU ignores. The output's HDR metadata is attached afterwards (§4).
- Compiled shaders are kept in `~/.cache/vidcut/shaders` (`cuda::keep_shaders_in`), so a new converter starts faster.

**Decoded on the GPU, made on the CPU** (`export::run_decoding_on`):
- While `gpu.acceleration` and `gpu.decoding` are on, an export's pieces decode on the GPU the viewer decodes on (`GpuDecoder::best`: NVDEC, VA-API or Direct3D 11), where it decodes their streams, whatever the encoder.
- They decode on a CUDA stream or device of their own (`GpuDecoder::own`), so their work and playback's don't wait in line behind each other.
- Pieces NVENC takes stay on the GPU, as above. Other frames come down to be made on the CPU. When they are shown smaller than they are, they are first shrunk on the GPU (`hw::Shrinker`: `scale_cuda` in the halving steps above, `scale_vaapi`, or the Direct3D 11 video processor, [preview.md](preview.md) §6).
- AMD's and Intel's encoders take them back up, except where frames stay on the GPU throughout (VAAPI and AMF, below).
- The report counts these frames (`gpu_decoded`).
- **`gpu.decoding` off** decodes every piece on the CPU, NVENC's too (`hw::Decoding::Cpu`), and the GPU only encodes. A GPU that also encodes has to decode, scale and encode in turn, so on a small GPU the CPU can be faster at decoding (choosing by itself: [todo](../todo/gpu-decoding.md)).

**Frames on the GPU (VAAPI):**
- Where the VAAPI encoder is on the render node of the VA-API decoder, it opens on that decoder's device (`encode::open_video`'s `shared`).
- A piece whose frames need only turning and fitting stays on the GPU from decoder to encoder (`VideoFrames::fit_on_gpu`, `hw::Fitter`):
  - `transpose_vaapi` turns them upright as the CPU's `transpose` and flips do (spec D33).
  - `scale_vaapi` scales them into the output's layout (10-bit sources into 8-bit output too).
  - `pad_vaapi` centers them on black, at even offsets as the CPU pads.
- Pieces in other colors than the output's (`color::kept`: BT.601, full range, HDR into SDR), interlaced ones (which come down for `bwdif`) and black are made on the CPU and uploaded. `gpu_frames` counts the frames that stayed on the GPU.

**Frames on the GPU (AMF):**
- AMF takes Direct3D 11 textures only from the frames context it was opened with. Where its adapter is the Direct3D 11 decoder's, it opens on frames made on that decoder's device (`encode::Upload::d3d11`).
- The video processor draws the pieces' frames into them (`hw::d3d11::Scaler::into`): turned upright (with its stream rotation, where it has one), scaled into the output's layout (10 bits into 8 too), and placed on a black background where the aspect differs.
- The same pieces as with VAAPI stay on the GPU, except 8-bit ones into 10-bit output: AMD's video processor refuses to draw a decoder's NV12 into P010 (`E_INVALIDARG`), though it draws a single texture's (the self-check's).
- A fit that fails leaves that file's frames to the CPU rather than failing the export.
- **Background:** AMD's driver reads a background color given as YCbCr as RGB, so it is given as RGB: 16 of 255 where the frames are told full range ([gotchas.md](../gotchas.md)), which is video-range black there, and 0 for PQ's studio range.
- 10-bit sources into 8-bit output come out slightly low: the video processor drops the low bits where the CPU rounds.

**Passes** (spec D45, which lists the settings and defaults; `video.passes`, `first_pass`, `multipass`, `lookahead`):
- **Two runs** (`Passes::TwoRuns`; x264, x265 and VP9): the first run feeds every frame to an encoder opened with `AV_CODEC_FLAG_PASS1`, drops its packets and writes only statistics. The second run opens the encoder again with `PASS2` and the statistics, and encodes as in one pass, with the audio.
- **Two inside one encode** (`Passes::TwoInside`): NVENC looks at each frame twice (`multipass`).
- VAAPI, AMF, QSV, ProRes and DNxHR make one pass. So does SVT-AV1 in FFmpeg 8.0: its wrapper ignores the pass flags, and `svtav1-params` can't hand the statistics back ([todo](../todo/svt-av1-two-passes.md)).
- Two passes asked of an encoder that can't make them is an error for that encoder, so `auto` moves on to one that can (from VAAPI, AMF or QSV to the CPU).
- **Statistics:**
  - x264 writes a file named by its `stats` option, plus its MB-tree file. x265 takes `pass` and `stats` in `x265-params`, with the path escaped (FFmpeg 8.0's wrapper ignores the flags), and writes a `.cutree` file beside it. Both finish their files when the encoder closes, so the first run's encoder is freed before the second opens.
  - libvpx returns its statistics in `stats_out` (base64, at the flush). vidcut keeps them in memory, copies them into an `av_malloc`'d `stats_in` for the second run, and frees both itself (FFmpeg doesn't).
  - The files live in a hidden `.<name>.passes` folder beside the output, on the output's disk: x264's and x265's per-block statistics grow to tens of kilobytes a frame at 4K, too much for a RAM-backed `/tmp`. The folder is removed when the export ends, however it ends.
- **`first_pass`:** `fast` (the default) is x264's `fastfirstpass`, x265 without `slow-firstpass`, and VP9 at speed 4 or faster in the first run. `full` gives the first run the second's settings.
- **NVENC:** `multipass` is `quarter` (`qres`, the default) or `full` (`fullres`); `full` costs time without a better result. `lookahead` (`rc-lookahead`) is 32 frames unless set, 0 for none, in both rate controls; it lifts quality for little time.
- **Progress** reports the run (`pass`, `passes`), each run counting its frames. The CLI's bar and a job's `progress` and `eta` cover both runs.
- Pieces NVDEC decodes stay on the GPU in NVENC's single run. The CPU encoders' two runs decode as a one-pass export does.
- **Loudness normalization** measures the mix before the second run, as before a one-pass export. Its measuring decodes only audio, so it isn't folded into the first run.
- `crates/vidcut-av/examples/passes.rs` compares pass settings by bitrate and PSNR.

**Details:**
- Quality 0 with libx264 means lossless, which needs the High 4:4:4 Predictive profile.
- SVT-AV1's info logging is silenced with `SVT_LOG=2`, which the binary sets at startup.

## 6. Quick Export semantics

- With no range given, an export covers the timeline's In–Out when both marks are set, otherwise the whole timeline (spec §10).
- With no preset given, the output extension picks one: `h264-master` settings for video containers (container from the extension), and the matching `audio-*` preset for audio extensions.
- **Per-clip export** writes one file per clip in the range, without the neighbors' J/L audio, named by a template (`{index:03}-{name}`).
- Existing files are kept unless `overwrite` is set.

## 7. Logging

FFmpeg's own logging defaults to fatal only: decoders recovering after a seek log harmless errors, and real failures surface as vidcut errors with context. `VIDCUT_AV_LOG` ([tools.md](../tools.md)) shows more.
