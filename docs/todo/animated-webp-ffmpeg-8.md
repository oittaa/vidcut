# An animated WebP with FFmpeg 8

**When:** not decided.

FFmpeg's animated WebP demuxer and decoder are new in 9.0. FFmpeg 8 reads an animated WebP, even one of one frame, with `webp_pipe` as one image it can't decode, so vidcut takes it as a still 2×2 pixels whose picture never renders (a render says it "ends before" the frame; checked with Ubuntu 26.04's FFmpeg 8.0.1). Refusing it on import, saying it takes FFmpeg 9.0, would be better than a still that shows nothing: the VP8X chunk's animation flag tells an animated WebP.
