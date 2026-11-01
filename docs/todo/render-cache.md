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

- Advanced has Folder, for a faster disk, for example.
- The quality values of Auto with proxies off are starting values. A measurement sets them. Its plan goes to the owner first.
- In an HDR timeline, the cache has 10 bits, in the PQ or HLG of the timeline. This needs a check.
- The frames that playback made, and that the memory still keeps, go to the cache first. Thus they are not made again.
  - The render finds the layers of a frame by the `Id` that playback gives them. An `Id` does not hold the place on the screen.
  - Thus the render places and mixes these layers as it does all layers. A cached frame keeps a name of its own.

## Differences from Resolve

The owner accepted these. Each goes into [deviations.md](../deviations.md) with the pull request that makes it.

- The background render first writes the frames that playback made.

## Steps

- **Codec, folder and writing:** all items of [Codec, folder and writing](#codec-folder-and-writing).
- **Dropped frames:** the measurement of the marks.

Each step adds its part to the spec, the design docs and the deviations. The last step deletes this file.
