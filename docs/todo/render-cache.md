# Render cache

**When:** next. Make one pull request for each item.

## Follow Resolve

The data comes from the manual, pp. 143–144 and 199–206.

- Keep the effects that you select in Render Cache Effects in the cache of the clip (pp. 202, 204). Apply the other effects of the clip during playback.
- Keep a still image in the cache of the clip.
- Keep a clip that plays backward in the cache of the clip.
- Keep the alpha channel in the cache files of ProRes 4444, ProRes 4444 XQ and DNxHR 444 (p. 201). The DNxHR encoder of FFmpeg has no pixel format with alpha.
- Then keep a clip with an alpha channel in the cache of the clip.
- Show a different bar on the ruler for each cache (p. 203).
- Add the window Playback > Manage Render Cache (p. 206).
- Find the cache that Resolve uses for speed changes and Retime Processes. Then use the same cache.
  - Pages 202 and 204 give different data. Thus, do a test in Resolve.

## Frames that playback made

- Use again the frames of a Retime Process that playback made, in the cache of the clip.
- In an HDR timeline, use again the AI pictures, the frames between two source frames and the Smooth Cuts that playback made.
  - The reader of the player makes them without the colors of the timeline (`with_timeline`).
