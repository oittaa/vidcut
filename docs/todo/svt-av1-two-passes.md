# Two passes with SVT-AV1

**When:** once vidcut's minimum FFmpeg has them (8.1 or later).

**Why:**

- AV1 on the CPU (libsvtav1) makes one pass (`encode::passes`, [design/export.md](../design/export.md#choosing-an-encoder)).
- FFmpeg 8.0's wrapper ignores `AV_CODEC_FLAG_PASS1`/`PASS2`.
- `pass=` in `svtav1-params` cannot work, because the statistics must go back between the passes.

**FFmpeg 8.1:**

- FFmpeg 8.1 and later (libavcodec 62.28.100, with SVT-AV1 2.0 or later) support two passes as libvpx does.
- The first pass gives base64 statistics in `stats_out` at the flush. The second pass takes them in `stats_in`.
- Two passes need VBR (a bitrate, no `rc_max_rate`, no CRF) and a closed GOP.
- The library makes its first pass faster by itself. Thus `first_pass` has no meaning there.

**To do:**

- `Passes::TwoRuns` with statistics in memory does the rest.
- Let `encode::passes` allow `libsvtav1` where libavcodec is new enough.
- Make the encoder set `AV_CODEC_FLAG_CLOSED_GOP`.
- Test it with `scripts/ffmpeg-check.sh release`.
