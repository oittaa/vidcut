# Export: decisions from M2

**Status:** accepted (M2, October 2026).

**Evidence:**
- [`crates/vidcut-av/tests/export.rs`](../../crates/vidcut-av/tests/export.rs): frame and sample exactness, A/V offset, rate conversion, all presets, cancellation, memory, and GPU encoders behind `--features vidcut-av/gpu-tests`.
- [`crates/vidcut/tests/cli.rs`](../../crates/vidcut/tests/cli.rs): export through the CLI.

## 1. In-process and streaming

**Where it runs:**
- Export runs in-process on the FFmpeg libraries (`vidcut-av::export`). There are no ffmpeg CLI subprocesses (a user decision).
- The work is planned by `vidcut_core::render::flatten`, a pure function, then encoded by the engine.

**Plan:**
- Video is a list of pieces: source ranges or black.
- Audio is one or more lanes of pieces: source ranges or silence.
- Every boundary is computed with exact integer math. Audio boundaries use `Rational::frame_to_sample` (i128, rounded), so lengths never drift. For example, 29.97 fps at 48 kHz alternates 1601 and 1602 samples per frame.

**Engine:**
- For each output frame, it pulls the next video frame from the current piece's decoder, then exactly the audio samples that frame spans from each lane, and encodes them.
- Memory stays flat with the segment count: each reader (video, and each audio lane) has its current file open, plus the next one while it opens. The test with 400 segments verifies this, in a process of its own.
- This replaced the plan's earlier idea of one big filter graph, which needed about 155 MB per input in measurements.
- A two-pass export (§5, Passes) runs the video through the encoder twice: the first time to measure, muxing nothing, then as above.

**Starting a piece** (`export/video.rs`, `export/audio.rs`, `export/ahead.rs`):
- **Same file as the piece before:** the reader reads on. A piece that starts at most 1 s past where the decoder is decodes on to its first frame (or skips samples); anything else seeks the open file. A frame repeated across the cut needs neither.
- **Another file:** opening it, seeking and decoding up to its first frame (up to a whole GOP, and round trips on a network share) happen on a helper thread while the piece before encodes. The piece then takes the opened source; a failure there is the same error the piece would have hit.
- **One opener thread per reader for the whole export**, not one per piece. glibc gives every new thread its own malloc arena, and decoder memory freed across many arenas stays resident. Per-piece threads raised the 400-segment test's peak from about 50 MB to 230 MB; one thread keeps it at about 100 MB (the second decoder).

**Output:** goes to a hidden `.<name>.part` file, renamed into place on success. Errors and cancellation delete it.

**Damaged sources:** a packet the decoder rejects as invalid data is skipped, as players and the `ffmpeg` tool skip it, and the export goes on.
- Video shows the next frame in its place (the sampling rule, §2). Frames that refer to it show what the decoder makes of them, up to the next keyframe.
- Audio has silence there (§3).
- Any other decoder error still fails the export. A hundred damaged frames in a row are still skipped; the 101st fails. Audio levels, scene measurements and waveforms decode by the same rule (`decode::decode_stream`).

## 2. Sampling rule (preview and export)

Output frame `k` of a source piece shows the **first source frame whose timestamp is at least `origin + (source_frame + k)/fps − ½ frame`**.

**What it gives:**
- Sources at the project rate: frame-exact, as verified by barcodes across H.264, HEVC 10-bit, VP9, ProRes, MPEG-TS and VFR fixtures.
- Other rates: nearest-frame sampling with ties going to the later frame.
- Example: a 25 fps clip on a 30 fps timeline shows source frames `0 1 2 3 3 4 5 6 7 8 8 9…`, 5 source frames per 6 output frames.

**Stills and image sequences (M9):**
- A still's one picture is decoded once and shows for every frame of its pieces. It is never sought: an image file has nothing past its picture.
- A sequence is read by FFmpeg's `image2` demuxer (`decode::Input::Sequence`): picture `k` has timestamp `k` in a time base of one frame at the sequence's rate, so the rule above picks pictures as from a video file, frame for frame at the project's rate and nearest-frame otherwise. The run is the contiguous files from its first number: image2's `start_number_range` is 1, so a missing first file fails the open instead of starting a few numbers later. The last picture lasts its own frame, as a video's does: at a project rate above the sequence's, the last output frame or two sample a time just past it. A run that ends before the sequence's count (a missing file) fails the export instead of repeating the picture before the hole.

**Preview vs export:**
- The preview (the player engine, [preview.md](preview.md)) reads with these same readers, so it shows the export's frames at every rate.

## 3. Audio

**Reading:**
- Each source piece is read sample-exactly: seek before the start, decode, find the first frame's timestamp, and drop samples up to the start time.
- Decoders get the packet time base (`pkt_timebase`). Without it, FFmpeg can't shift timestamps past skipped priming samples, and audio was found to start 6.5 ms late (Opus pre-skip, AAC priming).
- **Gaps follow the timestamps.** A frame that starts more than 2 ms after the one before it ended follows a gap: a packet the file lacks, or one the decoder rejected.
  - The gap becomes silence, so what follows stays in sync with the picture. Counting samples instead put everything after a lost AAC packet 21 ms early.
  - The silence is made a second at a time as it is read, so a jump in the timestamps costs no memory.
  - Audio that starts after a piece's start (a stream that begins after the picture, or a lost first packet) is silent until it does. Starting it at once put it early by that much.
  - A frame stamped past the file's end has a broken timestamp: it plays in order, with no gap. Otherwise one bad timestamp would silence the rest of the piece.
  - That end is the container's start plus its duration. The duration is a length. When the start is missing, the length is added to the audio stream's start, not counted from zero; with no start at all, the end counts as unknown (below). Timestamps on an offset clock would otherwise look past the end, a real gap would be dropped, and later audio would shift early.
  - When the duration is missing or not positive, a jump of more than a second plays in order too. A shorter one is still silence. Filling the whole jump would silence the rest of the piece, a second at a time, for as long as the timestamp says.
  - Smaller differences are timestamp rounding (Matroska keeps milliseconds).
  - Audio levels and waveforms count gaps the same way, so silence found in a measurement is where the export has it.

**Conversion and mixing:**
- libswresample converts to interleaved `f32` at the export rate and layout: mono is upmixed, 5.1 is downmixed. vidcut sizes the output buffers itself; ffmpeg-next's wrapper can truncate when upsampling.
- **Loudness normalization** (`audio.loudness`, M7): the mix is measured first (EBU R128 via FFmpeg's `ebur128`), then gained toward the target, held so true peaks stay at or below −1 dBTP (agents.md §5).
- Lanes are summed in Rust, with clip gain applied.
- **De-click:** 3 ms linear fades at every cut inside a lane (on by default; `declick=false` turns them off). Silence-removal cuts would click without them.

**Verified:** beeps land within 1 ms of where they should across codec boundaries, and output sample counts are exact.

## 4. Video normalization

- Each source frame goes through a libavfilter graph: `scale` (fit inside the output keeping the display aspect, honoring the sample aspect ratio), `pad` (black, centered), `setsar=1`, `format=<encoder pixel format>`.
- The fit is computed in Rust (`export::video::fit`, unit-tested).
- **Rotation** (spec D33): a stream's display matrix (phones store portrait video sideways with one) turns the picture upright first: `transpose=clock` or `cclock`, or `hflip,vflip` for 180°. The fit then uses the shown aspect. Thumbnails turn their pictures the same way.
- **Interlaced sources** (spec D34): `VideoReader` deinterlaces streams flagged interlaced, in decode order and before the sampling rule picks frames, so `bwdif` sees each frame's real neighbors.
  - It runs `bwdif=mode=send_frame:deint=interlaced`: one frame per frame, with the first field kept, and progressive frames passing through. Frames NVDEC decodes go through `bwdif_cuda` with the same options, on the GPU, the same way (§5).
  - A seek starts it over, and frames decoded on the way to the target feed it. `bwdif` halves its output time base, so timestamps are rescaled back to the stream's.
  - Thumbnails aren't deinterlaced: at pool size, combing barely shows.
- **Color** (`color.rs`, spec D35). Every picture is converted to the output's colors, from its tags or else what players assume (untagged: BT.709 when 1280 wide or over 576 lines, BT.601 otherwise, as players commonly guess and the viewer reads them; HDR untagged in BT.2020):
  - **SDR into SDR:** `scale` converts the YUV matrix and range to BT.709 limited as it scales. SD sources become BT.709, and full-range sources limited.
  - **HDR into SDR:** `zscale` takes the picture to linear light, with the reference white (203 nits) at 1.0, then to BT.709 primaries. `tonemap=mobius` (transition 0.8, peak 1000 nits) maps it, and `zscale` gives BT.709's transfer.
    - Everything up to 0.8 of the reference white stays as it was, so SDR-graded material in an HDR file looks as it did, and highlights roll off above it. PQ's 203 nits and HLG's 75% both come out at about 95% white.
    - FFmpeg's usual `hable` curve put that white at about 40% gray.
  - **Into HDR** (PQ or HLG): the same linear light, SDR white at 203 nits (BT.2408), then BT.2020 and the transfer. An SDR source's white lands at PQ code 573 of 1023, exactly 203 nits.
  - **Output tags:** encoders tag what they write: BT.709, or BT.2020 with PQ or HLG.
  - **Which output** (spec D44): `video.hdr = auto` (the default) and an unset `video.bit_depth` are decided per file before encoding, from the frames it shows (`vidcut_core::export::color`, a pure function, unit-tested): HDR when most of them are HDR and the codec carries it with a 10-bit encoder here, 10 bits for SDR when most have 10 bits or more and the codec isn't H.264. The session takes each media's transfer and pixel format from the project (`pixel_depth` asks FFmpeg's pixel format descriptor); the engine, given `auto` directly, probes the files. What the encoders can write comes from the encoder cache and the CPU encoders' pixel formats (`encode::abilities`): SDR takes 10 bits only from the encoder an 8-bit export starts with, so `auto` with an 8-bit-only GPU doesn't move a 10-bit SDR export to a slower CPU encoder, while HDR takes any 10-bit encoder. The plan's and the result's `outputs[].color` say what was chosen and why; `notes` carries the tone-mapping note (code `hdr_tone_mapped`) with the `set` overrides that keep the picture HDR.
  - **zimg detail:** `zscale` needs explicit output primaries when it converts to linear. Untagged frames otherwise give it no path.
  - **swscale only scales what `zscale` converts:** `scale` is told the source's matrix and range for both sides. Told nothing, it converted the matrix to BT.601 (the graph's source carries no colorspace), and `zscale` then read BT.601 numbers as BT.2020: saturated HDR colors came out up to 70 levels off, and HDR kept as HDR was rewritten in BT.601 under a BT.2020 tag. `zscale` writes the YUV itself, for NVENC's NV12 and P010 in their planar layouts, which swscale only interleaves: left to choose, it wrote RGB, and swscale's conversion to YUV came out over half a level brighter than zimg's.
  - **Cost at 4K60 with NVENC:** about 31 frames a second converting on the CPU, against 50 for plain SDR. On the GPU (§5) it converts at NVENC's pace: 71 frames a second into 2160p H.264.
  - **Real footage:** a 4K60 SDR clip exported as PQ and tone-mapped back differs from the original by about 6 of 255 on average.
- **HDR metadata** (spec D46, `video.hdr_metadata`, `mastering_display`, `max_cll`; `vidcut_av::hdr`, `vidcut_core::export::metadata`). Chosen per file after the dynamic range, from what the project knows of each piece (the probe reads it from the first decoded frame, where FFmpeg puts the container's values unless the bitstream has its own) or, given `auto` directly, from the files.
  - **Static values** go to the encoder before it opens (`AVCodecContext.decoded_side_data`): libx265 and libsvtav1 write them into the bitstream (x265 with `repeat-headers=1`, at every keyframe rather than only in `hvcC`; SVT-AV1 on every keyframe), NVENC when FFmpeg was built with its SDK 13 headers, and FFmpeg's `ff_encode_preinit` copies them into the stream's parameters, from which MP4/MOV (`mdcv`/`clli`) and Matroska/WebM write them, whichever encoder ran. Every frame carries them too, for VAAPI, which reads them only there. A keyframe packet still without them gets them from vidcut (`hdr::insert_static`: the mastering display and light level SEI, or AV1's HDR_MDCV and HDR_CLL metadata OBUs): Ubuntu's FFmpeg 8.0 builds NVENC without SDK 13, so its streams had none. x265 also gets `hdr10-opt=1` for PQ.
  - **Each output frame** loses its source's HDR metadata (the CPU's `scale` passes it on, and VAAPI and libvpx would write it) and gets back only the dynamic kinds the file keeps, from the frame it shows (`VideoFrames::with_dynamic`): after scaling, libplacebo or the upload, which drop or change it. The decoded frame's are kept apart as it is decoded: the GPU's scaler takes the frame itself (`av_buffersrc_add_frame` moves it), side data and all.
  - **HDR10+:** FFmpeg's HEVC and AV1 encoders don't write it (8.0 and 9.0; 9.0's libaom does), so vidcut puts it into their packets (`hdr::insert_hdr10plus`): FFmpeg's `av_dynamic_hdr_plus_to_t35` after the T.35 header Matroska also writes, in a prefix SEI before the first slice (escaped, at the slice's temporal id) or a metadata OBU before the first frame OBU. libvpx-vp9 takes it from the frames, and WebM/Matroska store it as BlockAdditions.
  - **Dolby Vision:** libx265 and libsvtav1 make an RPU from each frame's `AV_FRAME_DATA_DOVI_METADATA` when `dolbyvision=1` (profile 8 for HEVC with the base layer's compatibility from its tags, 10 for AV1); a frame without one is an error, which is why every picture must have it. x265 needs a VBV for it: peaks of 1.5 times a target bitrate, or 160 Mb/s at constant quality. MP4 writes `dvcC` only with `strict=unofficial`.
  - **Structs:** ffmpeg-sys-next doesn't bind `mastering_display_metadata.h`, so `hdr.rs` mirrors the two structs' fields and takes FFmpeg's allocators' sizes (they may grow). HDR10+ and Dolby Vision stay FFmpeg's opaque buffers, copied by reference.
  - **Tests** (`vidcut-av/tests/hdr.rs`): fixtures with HDR10 values and HDR10+ on every frame (`FixtureSpec::hdr_meta`, written as an export writes them) read back from the container, the bitstream alone and the probe; exports through x265 (MP4, MKV), SVT-AV1 (MP4, WebM) and VP9 (WebM) keep the values and each frame's HDR10+; mixed sources give the widest values; SDR none. The manifest's real clips: HDR10 values through x265, 4K AV1 with HDR10+ scaled down, Dolby Vision 8.4 into Matroska and MP4. With `gpu-tests`, hevc_nvenc, av1_nvenc and hevc_vaapi (where the machine has them) write the static values into the bitstream at every keyframe and the container, keep each frame's HDR10+ scaled on the GPU, and with an SDR clip that libplacebo converts, the widest values and no HDR10+.

## 5. Encoders

**Settings and presets:**
- Settings are pure data in `vidcut_core::export`: container, video (codec, encoder, quality or bitrate, speed, profile, bit depth, size, dynamic range) and audio (codec, bitrate, rate, channels). The dynamic range and an unset depth are chosen per file (§4, spec D44); presets leave them to it, except TikTok's, which asks for SDR.
- Built-in presets follow Resolve's Quick Export models: H.264/H.265 Master, ProRes 422 HQ, YouTube 720p/1080p/2160p, Vimeo, TikTok. There are also AV1, VP9, DNxHR HQ and five audio-only presets. The Quick Export row shows H.265 Master, AV1, ProRes 422 HQ, YouTube 1080p and 2160p, and TikTok (deviation X3).
- User presets are TOML files in `~/.config/vidcut/presets/`.
- `--set key=value` overrides any field.

**`auto` encoder:**
- Picks NVENC if a test encode works, then VAAPI, then the CPU encoder.
- An export takes the first of these that opens (`encode::video_encoders`). A GPU encoder that tested fine can still fail to open when an export starts: NVENC allows a few sessions at a time, and other exports may hold them. The report names the encoder used; the plan (`--dry-run`) names the first choice.
- An encoder asked for by name (`nvenc`, `vaapi`, `cpu`) fails the export if it won't open.
- An explicit `video.speed` goes to the encoder an export starts with. Encoders don't share speed names (x264's, NVENC's p1–p7, SVT-AV1's numbers), so one it moves on to uses its own default.
- If none opens, the error says why each didn't.
- Detection test-encodes every GPU encoder × pixel format × VAAPI render node (~2 s) and is cached in `~/.cache/vidcut/encoders.json`.
- The cache is keyed by FFmpeg versions, the NVIDIA driver, the kernel and the render nodes.

**On the dev machine:**
- NVENC: H.264, HEVC and AV1, in 8 and 10 bit.
- VAAPI: H.264 and HEVC on `renderD129` (AMD). Not on `renderD128` (the NVIDIA node), and no VAAPI AV1.
- All 8 working configurations export frame-exactly (`gpu-tests`).

**Frames on the GPU (NVENC):** all of it, and the GPU encoders themselves, only while the preferences' `gpu.acceleration` is on (the default). Off, `export.plan` and `export.start` set every video encoder choice to the CPU (`export::as_preferred`), so a preset naming NVENC still exports, in software, and proxies take x264 or SVT-AV1.
- NVENC is opened on frames in GPU memory (`cuda::CudaFrames`, NV12, or P010 for 10-bit output). Each source piece that the GPU shows as the CPU would decodes on NVDEC and scales with `scale_cuda` (`VideoFrames::with_gpu`); frames are picked by the sampling rule as on the CPU, so other rates repeat or skip GPU frames the same way.
- **A piece qualifies** when it is a video file NVDEC decodes. Interlaced ones are deinterlaced by the reader with `bwdif_cuda`, in decode order and before the sampling rule picks frames, as the CPU reader runs `bwdif` (§4): a deinterlacer behind the picking would see gaps at other rates and after seeks. bwdif_cuda's pictures differ from bwdif's by its rounding only, the frames being the same. A picture of another aspect is fitted and centered on black with `pad_cuda`, as the CPU pads, at even offsets as FFmpeg's `pad` rounds them for 4:2:0 (an odd one put the GPU's pictures a row lower); FFmpeg's `pad_cuda` takes no 10-bit frames, so in 10-bit output libplacebo places it (below). The source builds its GPU filters when it opens, and makes its first picture there too (on the thread that opens the next piece ahead), and takes the CPU if they won't build. Other pieces, stills and sequences decode and normalize on the CPU as before, and black frames are made there; they are uploaded to the encoder's frames. VAAPI and software encoders take frames from the CPU.
- **Turned pictures** (spec D33: phone video), **other matrices** (BT.601: SD, and VP8, which always signals BT.601; BT.2020 SDR) **and full range** go through libplacebo on Vulkan first (`cuda::CudaPictures`), at the source's size, turned upright with its `rotate` (clockwise, as the display matrix says): the CUDA filters neither turn pictures nor change their colors. In 10-bit output a picture of another aspect is also scaled and placed by libplacebo, in one pass with the rest: it can't clear a border (it clears with a copy FFmpeg's Vulkan frames don't allow), so it has two inputs, a black frame of the output's size under the picture, and nothing is left to clear. The Vulkan device is derived from the CUDA device once (a quarter of a second), and both are one for the whole process, shared by every export and proxy and never closed (`CudaDevice::open`): the Vulkan loader crashed when two exports made Vulkan instances at the same time; each frame is copied CUDA → Vulkan, converted, and copied back, never leaving the GPU (Vulkan → CUDA mapping isn't supported, copies are). The input is tagged with the matrix and range the CPU reads (`color::gpu_color`, untagged SD as BT.601) and with the output's primaries and transfer, so only the matrix and range change, as swscale converts. libplacebo is the more exact: on colored stripes it is within the encoder's rounding of the exact BT.601 → BT.709 numbers, where swscale is up to two levels low in luma.
- **HDR** (HDR sources into SDR, and anything into PQ or HLG) goes through libplacebo too, with the CPU's math (§4) in a user shader (`color::gpu_color`): PQ's EOTF or HLG's inverse OETF and 1.2 gamma channel by channel (zimg's approximate path, which `zscale` takes), linear light with the reference white at 1.0, the gamut matrix, the mobius curve on the brightest channel, negative channels black, and the output's transfer. libplacebo is told the picture is already in the output's primaries and transfer, so it only reads the matrix, scales, places and writes. The shader hooks the scaled picture, as the CPU scales before converting; libplacebo scales and places every HDR picture (the CUDA filters would scale converted pictures). HDR kept in the same transfer passes through the CUDA filters, as the CPU leaves it.
  - **libplacebo's own conversion can't give the CPU's curve:** measured on 640 patches (a gray ramp and 512 colors, PQ into SDR), its `mobius` with knee 0.8, peak detection and contrast recovery off and the 1000-nit peak put mid-grays up to 10 levels lower, as it lifts black for an SDR display's 1000:1 contrast (BT.1886 with a black level, which no option sets); HLG grays up to 6 levels off; and colors outside BT.709 up to 118 levels off, as it maps the curve on intensity (IPT) rather than the brightest channel and clips nothing, leaving them outside the video range.
  - **Half floats:** libplacebo keeps a picture between passes in half floats, and the GPU stores them rounded toward zero: up to almost half a 10-bit code low on PQ. The shader takes the middle of the step a value it reads came from, and moves what it writes half a step up; the GPU's pictures were half a level darker than the CPU's before.
  - **Agreement**, on the patches through NVENC against the CPU's lossless export: PQ into SDR grays within a level (on average 0.05 low in 8 bits, 0.2 in 10, where NVENC drops P016's low bits rather than rounding), colors within 1.2 levels but one: a cyan outside BT.709 at 1500 nits, whose clipped channel lands near zero, differs by 6 levels in Cr, the half floats' rounding magnified by the gamut matrix. HLG into SDR within 1.2, SDR into PQ and HLG and PQ into HLG within 0.7, PQ kept as PQ exact. On the 4K footage below, a frame's luma differs by 0.24 on average with the CPU's (2 or more in 0.13% of pixels), the same frame through both by FFmpeg's tools.
- **libplacebo's quirks:** FFmpeg's filter allocated without its Vulkan device converts on the CPU in front of it, so it is made by hand with the device set before it starts. It gives out a frame's picture only once it has the next frame; an export wants each picture at once, so a filler frame follows each frame and its pictures are dropped (proxies skip it: half the rendering). Its frame queue takes a frame within a millisecond of the render time as that time's frame, so the timestamps it sees are counts a second apart. A Vulkan device derived from CUDA keeps each plane of a frame in its own image, and libplacebo misses P010's 6-bit shift in such planes: it reads them 64 times too bright and writes blended pictures a 64th as bright. It gets 10-bit frames as P016, laid out the same with all 16 bits used, so they copy as they are; NVENC reads the top 10 bits. Its blue-noise dither lifts black by most of a 10-bit level, so it rounds instead. A frame's side data is dropped before it goes in: the filter copies the frame's properties to its picture and libplacebo turns both by their display matrices, so the turns cancel, and it would tone-map by HDR metadata and read ICC profiles, which the CPU ignores. The output's HDR metadata is attached after (§4). Compiled shaders are kept in `~/.cache/vidcut/shaders` (`cuda::keep_shaders_in`): a new converter starts in 20 ms instead of 0.1 s.
- **Black frames** (gaps, black pieces) in P010 keep their 10 bits high too: they were written low, a green-tinted black below black, in every 10-bit NVENC export.
- **Measured**, 60 s of VP8 (BT.601) 1920×800 through NVENC: 1438 of 1438 frames on the GPU, 15 CPU-seconds down to 3.7, the same wall time (5 s). 60 s of 1080p30 H.264 turned a quarter: 8.6 s and 10.6 CPU-seconds before, 7.1 s and 3.6 now (PSNR 48 dB against the CPU's). 30 s of 4K60 H.264 turned, to 2160×3840: 25 s either way (NVENC), 30 CPU-seconds down to 3.3. Into 1080p 10-bit HEVC with black bars: 60 s of 1280×534 H.264, 4.4 s and 16.3 CPU-seconds before, 4.3 s and 3.0 now (PSNR 55 dB against the CPU's); the VP8, 5.9 s and 27 before, 5.3 s and 4.2 now. 60 s of 4K60 10-bit AV1: to 1080p H.264 34.7 s and 129 CPU-seconds before, 16.4 s and 3.3 now; to 2160p 53.7 s and 149 before, 51.9 s and 3.9 now, NVENC's default preset (p5) setting the pace. 1080p H.264 sources: the same time (NVENC-bound), 8.3 CPU-seconds down to 3.2. HDR, on 30 s of the 4K60 clip made PQ and HLG with SDR white at 1000 nits (so most of the picture is above the knee): PQ into 1080p H.264 13.8 s and 137 CPU-seconds before, 7.1 s and 2.0 now (PSNR 48.5 dB against the CPU's); into 2160p 41.8 s and 418 before, 25.3 s and 3.4 now (51.0 dB); HLG into 1080p 14.3 s and 138 before, 7.1 s and 2.1 now (48.5 dB). Into HDR, 10-bit HEVC at 2160p: the SDR clip into PQ 24.3 s and 211 before, 19.6 s and 2.5 now (56.5 dB); PQ into HLG 25.8 s and 233 before, 17.8 s and 2.6 now (55.6 dB). 60 s of 1080i (made from the 4K60 clip, top field first) into 1080p H.264: 6.8 s and 10.8 CPU-seconds before, 6.7 s and 0.8 now (51.5 dB; the median frame 52.4, 3 of 1800 under 45).
- The report counts the frames made on the GPU (`gpu_frames`).
- **Tests:** `tests/export.rs` (`gpu-tests`) exports a plan of 720p H.264, 10-bit HEVC, 10-bit AV1, 60 and 24 fps sources, a seek back, black, an SD (BT.601) piece, pieces stored turned 90°, 180° and 270° (one in 10 bits), and 4:3 and 2.4:1 pieces that need black bars through NVENC in 8 and 10 bits: every frame's barcode is the CPU export's, every piece's frames but black were made on the GPU, and black is video-range black. Interlaced pieces among them (25 fps into 30, a seek back, and SD, which libplacebo converts after deinterlacing) pick the CPU's frames and don't comb. Colored 4:3 SD pictures (`Picture::Colors`) in 8 and 10 bits check the BT.601 conversion into both depths: each stripe within 1.5 levels of the exact numbers and 2 of the CPU's, and the black beside it. HDR: PQ, HLG stored turned, 4:3 PQ, colored PQ, SDR and colored SD pieces into 8 and 10-bit SDR, PQ and HLG: every frame but black made on the GPU, every barcode the CPU export's, the barcode cells' black and white and the stripes within two levels of the CPU's. Without the GPU, `hdr_colors_keep_their_matrix` checks PQ stripes into SDR against the math and kept as PQ against the source, and NV12 and P010 frames against the planar ones.

**Passes** (spec D45, `video.passes`, `first_pass`, `multipass`, `lookahead`):
- **What makes two:** x264, x265 and VP9 encode the range twice (`Passes::TwoRuns`): the first run feeds every frame to an encoder opened with `AV_CODEC_FLAG_PASS1`, drops its packets and writes nothing but statistics; the second opens the encoder again with `PASS2` and the statistics, and encodes as in one pass, with the audio. NVENC looks at each frame twice inside one encode (`multipass`, `Passes::TwoInside`). VAAPI and ProRes and DNxHR make one. SVT-AV1 makes one in FFmpeg 8.0: its wrapper ignores the pass flags, and `svtav1-params` can't hand the statistics back ([todo](../todo/svt-av1-two-passes.md)).
- **Unset `passes`:** 2 with a target bitrate, 1 with constant quality, where the encoder makes two; one elsewhere. Two asked for where an encoder can't: an error for that encoder, so `auto` moves on to one that can (VAAPI to the CPU). Two need a bitrate (validation): at constant quality a second pass changes nothing worth the time.
- **Statistics:** x264 writes a file it names with the `stats` option (and its MB-tree file); x265 takes `pass` and `stats` in `x265-params`, its path escaped (FFmpeg 8.0's wrapper ignores the flags), and writes a `.cutree` file beside it; both finish their files when the encoder closes, so the first run's encoder is freed before the second opens. libvpx hands its statistics back in `stats_out` (base64, at the flush); vidcut copies them into an `av_malloc`'d `stats_in` for the second, frees both itself (FFmpeg doesn't), and keeps them in memory. The files live in a hidden `.<name>.passes` folder beside the output, on the output's disk: x264's and x265's per-block statistics grow to tens of kilobytes a frame at 4K, too much for a RAM-backed `/tmp` on a long export. The folder is removed when the export ends, however it ends.
- **First pass (`first_pass`):** `fast` (the default) is x264's `fastfirstpass`, x265 without `slow-firstpass`, and VP9 at speed 4 or faster in the first run. `full` gives the first run the second's settings.
- **NVENC:** `multipass` `quarter` (the default, `qres`) or `full` (`fullres`) for the first look; `lookahead` (`rc-lookahead`) 32 frames unless set, 0 for none, in both rate controls.
- **Progress** reports the run (`pass`, `passes`), each counting its frames; the CLI's bar, a job's `progress` and `eta` cover both, and the Jobs window says "pass 1 of 2". Cancelling in either run stops it, and the `.part` file and the statistics go. A per-clip export makes both runs for each file. Pieces NVDEC decodes stay on the GPU in NVENC's single run; the CPU encoders' two runs take frames from the CPU, as in one.
- **Loudness normalization** measures the mix before the second run, as before the one; its measure decodes only audio, a small part of the time, so it isn't folded into the first run.
- **Measured** with `examples/passes.rs` (60 s stretches; bitrate from the file size; luma PSNR against the source frames as the export reads them): Tears of Steel 720p from 2:00 at 2 Mb/s, and Big Buck Bunny 1080p30 from 1:00 at 3 Mb/s. Times on the 16-thread dev machine.

| | 1 pass | 2, fast first | 2, full first |
|---|---|---|---|
| x264, ToS | +1.0%, 46.13 dB, 3.7 s | +0.5%, 46.14 dB, 4.7 s | +0.4%, 46.09 dB, 7.1 s |
| x264, BBB | −3.6%, 49.19 dB (worst frame 36.7), 8.0 s | −0.4%, 49.44 dB (39.4), 10.1 s | +1.1%, 49.50 dB (39.7), 13.9 s |
| x265, ToS | +2.5%, 47.20 dB, 16.7 s | +0.3%, 47.14 dB, 29.6 s | +0.3%, 47.14 dB, 32.8 s |
| x265, BBB | −2.6%, 50.01 dB (37.8), 33.0 s | −1.0%, 50.31 dB (42.1), 59.1 s | −0.1%, 50.32 dB (42.6), 60.6 s |
| VP9, ToS | +2.9%, 45.99 dB, 67.5 s | +0.1%, 46.67 dB, 118.5 s | the same (speed 4 already) |
| VP9, BBB | +28.3%, 49.34 dB (35.7), 150.6 s | −1.0%, 49.22 dB (36.2) on 28% fewer bits, 240.9 s | |

  Two passes land within about 1% of the target where one is off by up to 4% (x264, x265) and 28% (VP9), and lift the worst frames by up to 4.3 dB; they cost 25–35% more time with x264, 60–80% with x265 and VP9. The fast first pass saves a quarter to a third of x264's time (3–10% of x265's) for at most 0.06 dB. On a 15 s fixture of barcodes around noise at 800 kb/s, one pass overshoots by 87% (x264), 109% (x265) and 494% (VP9), two by 1.2%, 6.9% and 7.1% (`tests/export.rs`).
- **NVENC**, the same stretches (decoding on NVDEC sets the pace of H.264 and HEVC, so multipass costs them nothing):

| | one pass | quarter | full | one, lookahead 32 | quarter, lookahead 32 |
|---|---|---|---|---|---|
| H.264, BBB 3 Mb/s | +5.3%, 48.57 dB | +1.6%, 48.32 dB | +1.4%, 48.37 dB | +4.8%, 49.46 dB | +1.9%, 49.48 dB |
| HEVC, BBB 3 Mb/s | +5.4%, 48.52 dB | +2.8%, 48.46 dB | +3.1%, 48.44 dB, +10% time | +6.1%, 49.29 dB | +4.1%, 49.37 dB |
| AV1, BBB 3 Mb/s | +4.5%, 48.98 dB, 4.6 s | +4.0%, 48.90 dB, 4.8 s | +3.5%, 48.85 dB, 5.8 s | +3.8%, 49.81 dB | +3.3%, 49.63 dB |
| H.264, ToS 0.8 Mb/s | +5.1%, 43.71 dB | +5.1%, 43.71 dB | +5.1%, 43.71 dB | +4.6%, 43.82 dB | +4.5%, 43.83 dB |
| HEVC, ToS 0.8 Mb/s | +7.4%, 44.51 dB | +7.3%, 44.52 dB | +7.5%, 44.52 dB | +5.9%, 44.55 dB | +5.9%, 44.57 dB |

  Multipass brings the bitrate closer on the animation and changes little on the film; full resolution adds time for nothing more, so `quarter`. Looking 32 frames ahead adds 0.1 to 0.9 dB, costing time only in AV1 on the smaller picture (1.3 s to 1.6 s); at constant quality (H.264 and HEVC at 23, AV1 at 32) it also adds 0.7 to 0.8 dB on BBB with 2.5–4% smaller files, and 0.1 to 0.2 dB with 1% larger ones on ToS. So it is on by default, whatever the rate control. NVENC overshoots by 4–8% either way, its VBR allowed up to 1.5 times the target.

**Details:**
- Quality 0 with libx264 means lossless, which requires the High 4:4:4 Predictive profile.
- SVT-AV1's info logging is silenced through `SVT_LOG=2`, set by the binary at startup.

## 6. Quick Export semantics

- With no range given, export covers the timeline In–Out when both marks are set, otherwise the whole timeline (p. 789).
- With no preset given, the output extension picks one: `h264-master` settings for video containers (container from the extension), and the matching `audio-*` preset for audio extensions.
- **Per-clip export** writes one file per clip in the range, without neighbors' J/L audio, using a name template (`{index:03}-{name}`).
- Existing files are kept unless `overwrite` is set.

## 7. Logging

- FFmpeg's own logging defaults to fatal only. Decoders recovering after a seek log harmless errors, and real failures surface as vidcut errors with context.
- `VIDCUT_AV_LOG=error|warning|debug` shows more.
