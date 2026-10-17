# Two passes with SVT-AV1

**When:** once vidcut's minimum FFmpeg has them (8.1 or later).

AV1 on the CPU (libsvtav1) makes one pass (`encode::passes`, [design/export.md](../design/export.md#choosing-an-encoder)): FFmpeg 8.0's wrapper ignores `AV_CODEC_FLAG_PASS1`/`PASS2`, and `pass=` in `svtav1-params` can't work, since the statistics have to be handed back between the passes.

FFmpeg 8.1 and later (libavcodec 62.28.100, with SVT-AV1 2.0 or later) support it as libvpx does: the first pass gives base64 statistics in `stats_out` at the flush, the second takes them in `stats_in`. It needs VBR (a bitrate, no `rc_max_rate`, no CRF) and a closed GOP. The library runs its first pass faster by itself, so `first_pass` would mean nothing there.

`Passes::TwoRuns` with statistics in memory does the rest: `encode::passes` would allow `libsvtav1` where libavcodec is new enough, and the encoder would set `AV_CODEC_FLAG_CLOSED_GOP`. Test it with `scripts/ffmpeg-check.sh release`.
