# Subtitle pictures drawn ahead

**When:** not decided.

**Why:**

- An export draws each subtitle picture on its frame thread when the picture changes (`Burner` in `export/video.rs`).
- Decoding and encoding wait for the drawing.
- An animated track changes at each word and at each pop step.
- An example: 30 s of 1080×608 speech in the Pop look. The NVENC export takes 1.8 s, against 1.3 s without subtitles.
- Most of the difference is on one core: vidcut draws the text and builds NVENC's overlay in YUV.

**How:**

- vidcut knows the cues and their word times before the export starts.
- Thus worker threads can draw the pictures a few changes ahead. For NVENC, they also build the overlays.
- The workers give each picture to the frame thread when the frame thread gets to it.
- The player does the same: it draws titles and subtitles ahead on its decoding thread ([design/playback.md](../design/playback.md)).
- The pictures must not change. Compare them, frame for frame, with an export without the workers.
