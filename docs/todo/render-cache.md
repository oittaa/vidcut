# Render cache

**When:** next. Make one pull request for each item.

## Follow Resolve

The data comes from the manual, pp. 143–144 and 199–206.

- Keep the alpha channel in the cache files of DNxHR 444 (p. 201). The DNxHR encoder of FFmpeg has no pixel format with alpha.
- Keep a clip with an alpha channel in the cache of the clip when it keeps its placement in its job. Its file holds only the colors of the placed clip.
- Find the cache that Resolve uses for speed changes and Retime Processes. Then use the same cache.
  - Pages 202 and 204 give different data.
  - In a test, Resolve Studio 21.1.2 marked clips with Optical Flow and Frame Blend red. It did not render them. Find why, and do the test again.

## Frames that playback made

- Use again the frames of a Retime Process that playback made, in the cache of the clip.
- In an HDR timeline, use again the AI pictures, the frames that a Retime Process made and the Smooth Cuts that playback made.
  - The reader of the player makes them without the colors of the timeline (`with_timeline`).
