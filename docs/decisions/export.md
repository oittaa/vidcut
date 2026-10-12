# Export: decisions from M2

**Status:** accepted (M2, October 2026).

**Evidence:**
- [`crates/vidcut-av/tests/export.rs`](../../crates/vidcut-av/tests/export.rs): frame and sample exactness, A/V offset, rate conversion, all presets, cancellation, memory, and GPU encoders behind `--features vidcut-av/gpu-tests`.
- [`crates/vidcut/tests/cli.rs`](../../crates/vidcut/tests/cli.rs): export through the CLI.

## 1. In-process, streaming, one pass

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
  - **Into HDR** (`video.hdr = pq` or `hlg`): the same linear light, SDR white at 203 nits (BT.2408), then BT.2020 and the transfer. An SDR source's white lands at PQ code 573 of 1023, exactly 203 nits.
  - **Output tags:** encoders tag what they write: BT.709, or BT.2020 with PQ or HLG.
  - **zimg detail:** `zscale` needs explicit output primaries when it converts to linear. Untagged frames otherwise give it no path.
  - **swscale only scales what `zscale` converts:** `scale` is told the source's matrix and range for both sides. Told nothing, it converted the matrix to BT.601 (the graph's source carries no colorspace), and `zscale` then read BT.601 numbers as BT.2020: saturated HDR colors came out up to 70 levels off, and HDR kept as HDR was rewritten in BT.601 under a BT.2020 tag. `zscale` writes the YUV itself, for NVENC's NV12 and P010 in their planar layouts, which swscale only interleaves: left to choose, it wrote RGB, and swscale's conversion to YUV came out over half a level brighter than zimg's.
  - **Cost at 4K60 with NVENC:** about 31 frames a second converting on the CPU, against 50 for plain SDR. On the GPU (§5) it converts at NVENC's pace: 71 frames a second into 2160p H.264.
  - **Real footage:** a 4K60 SDR clip exported as PQ and tone-mapped back differs from the original by about 6 of 255 on average.
  - **Not done:** mastering-display and content-light metadata aren't written. Players then assume their own peak.

## 5. Encoders

**Settings and presets:**
- Settings are pure data in `vidcut_core::export`: container, video (codec, encoder, quality or bitrate, speed, profile, bit depth, size) and audio (codec, bitrate, rate, channels).
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
- **libplacebo's quirks:** FFmpeg's filter allocated without its Vulkan device converts on the CPU in front of it, so it is made by hand with the device set before it starts. It gives out a frame's picture only once it has the next frame; an export wants each picture at once, so a filler frame follows each frame and its pictures are dropped (proxies skip it: half the rendering). Its frame queue takes a frame within a millisecond of the render time as that time's frame, so the timestamps it sees are counts a second apart. A Vulkan device derived from CUDA keeps each plane of a frame in its own image, and libplacebo misses P010's 6-bit shift in such planes: it reads them 64 times too bright and writes blended pictures a 64th as bright. It gets 10-bit frames as P016, laid out the same with all 16 bits used, so they copy as they are; NVENC reads the top 10 bits. Its blue-noise dither lifts black by most of a 10-bit level, so it rounds instead. A frame's side data is dropped before it goes in: the filter copies the frame's properties to its picture and libplacebo turns both by their display matrices, so the turns cancel, and it would tone-map by HDR metadata and read ICC profiles, which the CPU ignores. Compiled shaders are kept in `~/.cache/vidcut/shaders` (`cuda::keep_shaders_in`): a new converter starts in 20 ms instead of 0.1 s.
- **Black frames** (gaps, black pieces) in P010 keep their 10 bits high too: they were written low, a green-tinted black below black, in every 10-bit NVENC export.
- **Measured**, 60 s of VP8 (BT.601) 1920×800 through NVENC: 1438 of 1438 frames on the GPU, 15 CPU-seconds down to 3.7, the same wall time (5 s). 60 s of 1080p30 H.264 turned a quarter: 8.6 s and 10.6 CPU-seconds before, 7.1 s and 3.6 now (PSNR 48 dB against the CPU's). 30 s of 4K60 H.264 turned, to 2160×3840: 25 s either way (NVENC), 30 CPU-seconds down to 3.3. Into 1080p 10-bit HEVC with black bars: 60 s of 1280×534 H.264, 4.4 s and 16.3 CPU-seconds before, 4.3 s and 3.0 now (PSNR 55 dB against the CPU's); the VP8, 5.9 s and 27 before, 5.3 s and 4.2 now. 60 s of 4K60 10-bit AV1: to 1080p H.264 34.7 s and 129 CPU-seconds before, 16.4 s and 3.3 now; to 2160p 53.7 s and 149 before, 51.9 s and 3.9 now, NVENC's default preset (p5) setting the pace. 1080p H.264 sources: the same time (NVENC-bound), 8.3 CPU-seconds down to 3.2. HDR, on 30 s of the 4K60 clip made PQ and HLG with SDR white at 1000 nits (so most of the picture is above the knee): PQ into 1080p H.264 13.8 s and 137 CPU-seconds before, 7.1 s and 2.0 now (PSNR 48.5 dB against the CPU's); into 2160p 41.8 s and 418 before, 25.3 s and 3.4 now (51.0 dB); HLG into 1080p 14.3 s and 138 before, 7.1 s and 2.1 now (48.5 dB). Into HDR, 10-bit HEVC at 2160p: the SDR clip into PQ 24.3 s and 211 before, 19.6 s and 2.5 now (56.5 dB); PQ into HLG 25.8 s and 233 before, 17.8 s and 2.6 now (55.6 dB). 60 s of 1080i (made from the 4K60 clip, top field first) into 1080p H.264: 6.8 s and 10.8 CPU-seconds before, 6.7 s and 0.8 now (51.5 dB; the median frame 52.4, 3 of 1800 under 45).
- The report counts the frames made on the GPU (`gpu_frames`).
- **Tests:** `tests/export.rs` (`gpu-tests`) exports a plan of 720p H.264, 10-bit HEVC, 10-bit AV1, 60 and 24 fps sources, a seek back, black, an SD (BT.601) piece, pieces stored turned 90°, 180° and 270° (one in 10 bits), and 4:3 and 2.4:1 pieces that need black bars through NVENC in 8 and 10 bits: every frame's barcode is the CPU export's, every piece's frames but black were made on the GPU, and black is video-range black. Interlaced pieces among them (25 fps into 30, a seek back, and SD, which libplacebo converts after deinterlacing) pick the CPU's frames and don't comb. Colored 4:3 SD pictures (`Picture::Colors`) in 8 and 10 bits check the BT.601 conversion into both depths: each stripe within 1.5 levels of the exact numbers and 2 of the CPU's, and the black beside it. HDR: PQ, HLG stored turned, 4:3 PQ, colored PQ, SDR and colored SD pieces into 8 and 10-bit SDR, PQ and HLG: every frame but black made on the GPU, every barcode the CPU export's, the barcode cells' black and white and the stripes within two levels of the CPU's. Without the GPU, `hdr_colors_keep_their_matrix` checks PQ stripes into SDR against the math and kept as PQ against the source, and NV12 and P010 frames against the planar ones.

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
