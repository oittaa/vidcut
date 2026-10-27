# Subtitle pictures drawn ahead

**When:** not decided.

**Why:**

- An export draws each subtitle picture on its frame thread when the picture changes (`Subtitles::draw_at`).
- Decoding and encoding wait for the drawing.
- An animated track changes at each word and at each pop step.

**How:**

- vidcut knows the cues and their word times before the export starts.
- Thus worker threads can draw the pictures a few changes ahead.
- The workers give each picture to the frame thread when the frame thread gets to it.
- The player does the same: it draws titles and subtitles ahead on its decoding thread ([design/playback.md](../design/playback.md)).
- The pictures must not change. Compare them, frame for frame, with an export without the workers.
- Measure the time of an export with a Pop track before and after, with a plan that the owner accepts first.
