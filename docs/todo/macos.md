# macOS

**When:** after the interchange and the smaller items ([roadmap](../roadmap.md)).

- `scripts/check.sh full` on a Mac.
- ProRes proxies on VideoToolbox checked on a Mac with ProRes hardware and on one without.
- An `.app` bundle with FFmpeg's libraries in it, signed.
- VideoToolbox's ProRes 4444 with alpha shown from GPU memory. It comes as AYUV64, its samples packed in one plane, which the viewer's shader doesn't read, so it comes down.
