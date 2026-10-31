# Render cache

**When:** next, one pull request for each step in [Steps](#steps), in their order.

The render cache renders the slow parts of the timeline in the background ([spec D98](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions), [playback](../design/playback.md#the-render-cache)). It keeps the frame after the tracks mix. This file tells what is still to do.

## Resolve

The manual describes the render cache on pp. 143–144 and 199–206.

- Resolve caches the checked Resolve FX and OFX of a clip in its node cache, before the tracks mix.
- The render cache format of a new project in Resolve Studio 21.1.2 is ProRes 422 HQ (`apch`).

## One record of each frame

Each kept layer has a `player::Origin` ([playback](../design/playback.md#the-engine)). It tells what made the layer, the upscaler, and when. The render cache uses these records. It adds no second record of how a frame is made.

- **The kept picture:** a cached part keeps the `Id` of the frames that it replaces.
  - Thus kept pictures, marks and Stats for Nerds name the same frame.
- **What it cost:** the `Origin` also holds the time that each layer took to decode and to enlarge.
  - It holds this time for a Retime Process and a Smooth Cut already.
  - The marks of dropped frames read these times.

## Dropped frames

Smart also renders parts that this computer cannot play. One late frame does not mark a part. A part gets a mark only when it drops frames steadily for a long time:

- The play is forward, at normal speed.
- Frames drop in each second of a stretch of at least 5 s.
  - In each of these seconds, a quarter of the frames or more come late or are skipped (`late`, `skipped`).
- The frames of the stretch took longer to make than their frame time, by the times in `Origin`.
  - Why: then the frames themselves are slow. A late draw on the screen does not mark a part.
- These times do not count:
  - the first second after a start or a seek
  - while vidcut runs a job or the background render
  - while the window is hidden
- The mark covers the part of the plan that holds the stretch (`VideoPiece`), by the names of its frames.
  - Thus a moved part keeps its mark. An edit that changes the frames makes a new part without a mark.
- The marks are in the index of the cache, on this computer. Delete Render Cache > All removes them too.
- The numbers above are starting values. A measurement changes them, with a plan that the owner accepts first.

## Codec, folder and writing

- Advanced has Codec and Folder.
  - The codec preference is Auto, or a codec of a list: the ProRes and DNxHR grades that Resolve offers, HEVC and AV1.
  - A chosen codec goes to the encoder as it is. If this computer cannot make it, Preferences says so, and the cache stays empty. No other codec takes its place.
  - The folder is for a faster disk, for example.
- The quality values of Auto with proxies off are starting values. A measurement sets them. Its plan goes to the owner first.
- In an HDR timeline, the cache has 10 bits, in the PQ or HLG of the timeline. This needs a check.
- The frames that playback made, and that the memory still keeps, go to the cache first. Thus they are not made again.
- The background render uses the AI model that the viewer has loaded. It does not load a second copy.
- `cache.render` (new): renders the cache without a GUI, for scripts.

## Differences from Resolve

The owner accepted these. Each goes into [deviations.md](../deviations.md) with the pull request that makes it.

- The background render first writes the frames that playback made.
- Smart also renders the parts where frames drop steadily.

## Steps

- **One record of each frame:** the kept `Id`.
- **Codec, folder and writing:** all items of [Codec, folder and writing](#codec-folder-and-writing).
- **Dropped frames:** the times of each layer in `Origin`, and the marks.

Each step adds its part to the spec, the design docs and the deviations. The last step deletes this file.
