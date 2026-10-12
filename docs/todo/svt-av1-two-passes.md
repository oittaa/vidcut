# Two passes with SVT-AV1

**When:** once vidcut's minimum FFmpeg has them (8.1 or later).

AV1 on the CPU (libsvtav1) makes one pass ([export.md §5](../decisions/export.md), Passes): FFmpeg 8.0's wrapper ignores `AV_CODEC_FLAG_PASS1`/`PASS2`, and passing `pass=` in `svtav1-params` can't work, since the statistics have to be handed back between the passes.

FFmpeg 8.1 and later (libavcodec 62.28.100, with SVT-AV1 2.0 or later) support it the way libvpx does: the first pass gives base64 statistics in `stats_out` at the flush (the wrapper doesn't free them), the second takes them in `stats_in`. It needs VBR (a bitrate, no `rc_max_rate`, no CRF) and a closed GOP. The library runs its first pass faster by itself, so `first_pass` would mean nothing there.

The engine's `TwoRuns` with statistics in memory already does the rest: `encode::passes` would allow `libsvtav1` when the libavcodec version is new enough, and the encoder would set `AV_CODEC_FLAG_CLOSED_GOP`. Test it with `scripts/ffmpeg-check.sh release`.
