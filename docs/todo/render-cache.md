# Render cache

**When:** next, one pull request for each step in [Steps](#steps), in their order.

The render cache renders the slow parts of the timeline in the background ([spec D98](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions), [playback](../design/playback.md#the-render-cache)). It keeps the frame after the tracks mix. This file tells what is still to do.

## Resolve

The manual describes the render cache on pp. 143–144 and 199–206.

- Resolve caches the checked Resolve FX and OFX of a clip in its node cache, before the tracks mix.
- The render cache format of a new project in Resolve Studio 21.1.2 is ProRes 422 HQ (`apch`).

## Dropped frames

- The numbers of the marks are starting values ([playback](../design/playback.md#the-render-cache)). A measurement changes them, with a plan that the owner accepts first.

## Codec, folder and writing

- The quality values of Auto with proxies off are starting values. A measurement sets them. Its plan goes to the owner first.
- In an HDR timeline, the cache has 10 bits, in the PQ or HLG of the timeline. This needs a check.
- At the clip level, the render makes again the frames that playback made.
  - A job of the clip level draws the colors of one layer (`colors_only`). A kept picture holds the whole frame, as the viewer draws it.
  - The `Id` of a made picture holds the size and the placement of the timeline. The plan of a job of the clip level has other values.
- In a piece that is cached in part, the render makes again the frames that playback made with AI Super Scale or a Retime Process.
  - `Cache::apply` cuts the piece of the viewer. The cut moves the keyframe position of the placement in the `Id` (`Placed::after`).
  - A fix: the `Id` holds the keyframe position of each frame, not that of its piece. This changes the `Id` of the player too.
- In an HDR timeline, the render makes again the frames that playback made.
  - The reader of the player makes its pictures without the colors of the timeline (`with_timeline`). The render makes them with these colors.

## Steps

- **Codec, folder and writing:** all items of [Codec, folder and writing](#codec-folder-and-writing).
- **Dropped frames:** the measurement of the marks.

Each step adds its part to the spec, the design docs and the deviations. The last step deletes this file.
