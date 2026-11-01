# Render cache

**When:** next, one pull request for each step in [Steps](#steps), in their order.

The render cache renders the slow parts of the timeline in the background ([spec D98](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions), [playback](../design/playback.md#the-render-cache)). It keeps the frame after the tracks mix. This file tells what is still to do.

## Resolve

The manual describes the render cache on pp. 143–144 and 199–206.

- Resolve caches the checked Resolve FX and OFX of a clip in its node cache, before the tracks mix.
- The render cache format of a new project in Resolve Studio 21.1.2 is ProRes 422 HQ (`apch`).

## Codec, folder and writing

- At the clip level, the render makes again the frames of a Retime Process that playback made.
  - The viewer keeps such a frame for the part of the timeline frame where the clip shows, with its placement (`Sampling::Placed`).
  - A job of the clip level makes the colors of the layer over the whole frame (`Look::whole`), with no edges and no alpha.
- A job does not take a kept frame in which a layer plays from its cache file.
  - Playback plays the plan after `Cache::apply`. There, `from_file` names that layer by its cache file.
  - `Kept` names the frames from the plan before `Cache::apply`. Thus the `Id` of such a frame does not match.
- At the clip level, a job does not take some kept AI Super Scale frames of anamorphic media.
  - `ai_picture` rounds the display width to the nearest even number. `display_size` rounds it to a whole number, then down to an even number. `ai_size` gets that size.
  - Example: 720×480 with a pixel aspect of 32:27 gives 854 in `ai_picture` and 852 in `display_size`.
  - Thus the enlarged sizes are different, and `Kept::at` does not take the frame.
- In an HDR timeline, the render makes again the frames that playback made.
  - The reader of the player makes its pictures without the colors of the timeline (`with_timeline`). The render makes them with these colors.

## Steps

- **Codec, folder and writing:** all items of [Codec, folder and writing](#codec-folder-and-writing).

Each step adds its part to the spec, the design docs and the deviations. The last step deletes this file.
