# Alpha in more render cache formats

**When:** once FFmpeg's DNxHR encoder has a pixel format with alpha.

**Why:**

- Resolve keeps the alpha channel of a cached clip in Uncompressed 10-bit, Uncompressed 16-bit Float, ProRes 4444, ProRes 4444 XQ and DNxHR 444 (p. 201).
- vidcut keeps it only in ProRes 4444 and ProRes 4444 XQ ([X106](../deviations.md#x106-a-clip-with-an-alpha-channel-is-cached-with-the-tracks-mixed)).
- The DNxHR encoder of FFmpeg has no pixel format with alpha. vidcut has no uncompressed codec for the cache.

**To do:**

- Cache media with an alpha channel at the clip level in DNxHR 444 when the encoder takes alpha.
- Add the uncompressed formats with alpha to the cache. Then X106 goes.
