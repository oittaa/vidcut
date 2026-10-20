# Subtitle pictures drawn ahead

**When:** not decided.

An export draws each subtitle picture on its frame thread when the picture changes (`Burner` in `export/video.rs`), so decoding and encoding wait for it. An animated track changes at every word and pop step: on 30 s of 1080×608 speech in the Pop look, the NVENC export takes 1.8 s against 1.3 s without subtitles, most of the difference drawing the text and building NVENC's overlay in YUV on one core.

The cues and their word times are known before the export starts, so the pictures, and for NVENC the overlays, can be drawn on worker threads a few changes ahead and handed to the frame thread as it reaches them, as the player draws titles and subtitles ahead on its decoding thread ([design/playback.md](../design/playback.md)). The pictures must stay the ones drawn now: check them against an export without the workers, frame for frame.
