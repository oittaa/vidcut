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

## 2. Sampling rule (preview and export)

Output frame `k` of a source piece shows the **first source frame whose timestamp is at least `origin + (source_frame + k)/fps − ½ frame`**.

**What it gives:**
- Sources at the project rate: frame-exact, as verified by barcodes across H.264, HEVC 10-bit, VP9, ProRes, MPEG-TS and VFR fixtures.
- Other rates: nearest-frame sampling with ties going to the later frame.
- Example: a 25 fps clip on a 30 fps timeline shows source frames `0 1 2 3 3 4 5 6 7 8 8 9…`, 5 source frames per 6 output frames.

**Preview vs export:**
- Preview (mpv EDL, see [preview.md](preview.md)) follows the same rule through the half-frame bias in segment starts.
- At mismatched rates, mpv's 5 ms hr-seek tolerance can make a paused preview differ from the export by one frame at exact ties. Accepted.

## 3. Audio

**Reading:**
- Each source piece is read sample-exactly: seek before the start, decode, find the first frame's timestamp, and drop samples up to the start time.
- Decoders get the packet time base (`pkt_timebase`). Without it, FFmpeg can't shift timestamps past skipped priming samples, and audio was found to start 6.5 ms late (Opus pre-skip, AAC priming).

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
  - It runs `bwdif=mode=send_frame:deint=interlaced`: one frame per frame, with the first field kept, and progressive frames passing through.
  - A seek starts it over, and frames decoded on the way to the target feed it. `bwdif` halves its output time base, so timestamps are rescaled back to the stream's.
  - Thumbnails aren't deinterlaced: at pool size, combing barely shows.
- **Not yet handled:**
  - HDR: needs tone mapping or passthrough decisions.

  These follow when real-world media tests need them; they are tracked in AGENTS.md.

## 5. Encoders

**Settings and presets:**
- Settings are pure data in `vidcut_core::export`: container, video (codec, encoder, quality or bitrate, speed, profile, bit depth, size) and audio (codec, bitrate, rate, channels).
- Built-in presets mirror Resolve's Quick Export: H.264/H.265 Master, ProRes 422 HQ, YouTube 720p/1080p/2160p, Vimeo, TikTok. There are also AV1, VP9, DNxHR HQ and five audio-only presets.
- User presets are TOML files in `~/.config/vidcut/presets/`.
- `--set key=value` overrides any field.

**`auto` encoder:**
- Picks NVENC if a test encode works, then VAAPI, then the CPU encoder.
- Detection test-encodes every GPU encoder × pixel format × VAAPI render node (~2 s) and is cached in `~/.cache/vidcut/encoders.json`.
- The cache is keyed by FFmpeg versions, the NVIDIA driver, the kernel and the render nodes.

**On the dev machine:**
- NVENC: H.264, HEVC and AV1, in 8 and 10 bit.
- VAAPI: H.264 and HEVC on `renderD129` (AMD). Not on `renderD128` (the NVIDIA node), and no VAAPI AV1.
- All 8 working configurations export frame-exactly (`gpu-tests`).

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
