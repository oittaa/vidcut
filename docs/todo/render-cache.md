# Render cache

**When:** next, one pull request for each step in [Steps](#steps).

Some frames take longer to make than the viewer shows them. The viewer then cannot play them in real time. These frames are such:

- **Retime Process with an AI process:** a 1080p clip plays at 25% on an M4 with macOS 27. The viewer shows 48% of its frames in time. In place of the others, it shows the frame before.
- **Super Scale with an AI upscaler:** on the same Mac, AI of Super Scale takes 474 ms for a frame of 540p shown at 1080p.
- **Many layers and effects on a slow GPU:** dissolves, clips over each other with an opacity, and clip effects such as Gaussian Blur ([spec §8.16](../cut-page-spec.md#816-effects-pp-765-3526-3536)).

The player keeps the frames that it made in memory, while its budget lasts. A longer part of the timeline does not fit.

A render cache makes the slow parts in the background and keeps them on disk. The viewer then plays them from there. Exports do not use the cache.

## Resolve

The manual describes the render cache on pp. 143–144 and 199–206. The chapters of the Cut page do not describe it. Thus the spec entry comes from these pages, as for proxies (D41).

- Playback > Render Cache has three modes: Off, Smart and User.
- Smart caches these:
  - clips in formats that are slow to decode: H.264, H.265, DCP, JPEG 2000 and camera raw
  - speed effects, at the level of the source. Thus a moved clip keeps its cache.
  - Fusion effects
  - transitions, opacity, and composite modes other than Normal
- User caches the clips set to On. It also caches the kinds that the project settings turn on: transitions, composites and Fusion effects.
- Each clip has Render Cache Fusion Output: Auto, On or Off. Off keeps the clip out of Smart too.
- Resolve FX and OFX on a clip are cached only when the user picks them, in Smart and in User mode.
  - The menu of a clip has Render Cache OFX Filter, with each filter of the clip.
- A track header has Exclude Track from Caching.
- Resolve caches the parts to cache while they play.
- It also caches in the background, after a number of seconds without changes.
- A line under the timeline ruler shows the state. Red is to be cached, and blue is cached.
- Delete Render Cache has All, Unused and Selected Clips.
- Each project has its own cache folder. A preference deletes cache files after a number of days.

A new project in Resolve Studio 21.1.2 on the owner's computer has these settings (read with the scripting API):

| Setting | Value |
|---|---|
| Render cache mode | Off (`none`) |
| Enable background caching | on |
| After | 5 s |
| Transitions in User mode | off |
| Composites in User mode | off |
| Fusion effects in User mode | on |
| Render cache format | ProRes 422 HQ (`apch`) |

## Modes

- The project has a mode: Off, Smart or User. A new project has Off, as in Resolve.
- A new Playback menu holds two submenus:
  - Render Cache: Off, Smart and User
  - Delete Render Cache: All, Unused and Selected Clips
- Each clip has Render Cache: Auto, On or Off. Off keeps the clip out of both modes.
- The menu of a clip also has Render Cache Effects, with each effect of the clip.
  - A checked effect renders in Smart and in User mode, as in Resolve.
- A track can be out of the cache (Exclude Track from Caching).

## When it renders

- The background render starts after a time without activity: 5 s, unless set.
- Activity is an edit, a play, a seek, a scrub, a trim or a drag.
- Each activity stops the render at once. The render continues later from where it stopped.
- It renders from the playhead forward, then the rest of the timeline.
- It is a job of the lowest priority. It yields to playback (`first::Claim`), and to exports, proxies and transcriptions (`jobs::Turns`).
- It renders on battery power too. The user can turn it off.
- Playback does not write the cache. Why: the writes would take GPU time from the frames on the screen.
- The frames that playback made, and that the memory still keeps, go to the cache first. Thus they are not made again.

## What Smart renders

| Resolve Smart | vidcut Smart |
|---|---|
| H.264, H.265, DCP, JPEG 2000, raw | the media that the proxy rules pick (`Proxies::wants`) and that play the original, and the media that decodes on the CPU (`GpuDecoder::why_not`) |
| speed effects | each clip with a speed change, and each clip whose frames a Retime Process other than Nearest makes |
| Fusion effects | AI Super Scale |
| Resolve FX and OFX, only the picked ones | the same: only the effects checked in Render Cache Effects |
| transitions, opacity, composite modes | the same: each transition, an opacity below 100%, a composite mode other than Normal |
| none | parts where this computer drops frames steadily ([Dropped frames](#dropped-frames)) |

Why not the list of formats: vidcut decodes H.264 and H.265 on the GPU. With the list of Resolve, Smart renders most clips of a camera or a phone. At 4K in ProRes 422 HQ, that is many GB for each minute.

User mode renders these:

- the clips set to On
- the kinds that Preferences turns on: Transitions (off), Composites (off) and AI (on)
  - Composites includes opacity, as in Resolve.
  - AI is AI Super Scale and the AI Retime Processes. It stands in for the Fusion effects of Resolve.

## The line on the timeline

- In Smart and User mode, a thin line runs under the ruler of both timelines.
  - Red: the mode renders this part.
  - Blue: the part is in the cache.
- The line changes after each edit, and while the render runs. In Off mode, there is no line.
- The pointer over a red part shows why the mode renders it, for example AI Super Scale or Transition.

## Size and codec

- With proxies on, the cache has the size and the codec of proxies (`proxies.size`, `proxies.codec`). It is never larger than the timeline.
- With proxies off, the cache has the size of the timeline.
- The codec preference is Auto, or a codec of a list.
  - Auto with proxies off: ProRes 422 HQ where the GPU makes it, as in Resolve. Else HEVC or AV1 of 10 bits at a high quality, on the GPU.
  - The exact quality of Auto needs a measurement. Its plan goes to the owner first.
  - The list: the ProRes and DNxHR grades that Resolve offers, HEVC and AV1.
  - A chosen codec goes to the encoder as it is. If this computer cannot make it, Preferences says so, and the cache stays empty. No other codec takes its place.
- In an HDR timeline, the cache has 10 bits, in the PQ or HLG of the timeline.
- The name of each cached frame holds the size and the codec. Thus the cache of each proxy setting stays, and a switch of proxies finds its cache at once.

## Two levels

1. **The clip level:** the picture of one clip, before the tracks mix.
   - It is for AI Super Scale, a Retime Process, a speed change and media that is slow to decode.
   - An edit on another track, a move, a split or a trim keeps the cache of the clip.
   - A frame of a Retime Process is made at its placement, with the effects of the clip ([export](../design/export.md#effects)).
   - Thus a change of the Transform or of an effect of that clip renders it again.
2. **The timeline level:** the frame after the tracks mix.
   - It is for transitions, opacity, composite modes, the checked effects and the parts with dropped frames.
   - Titles and subtitles on top of all tracks stay out of it. Thus an edit of their text keeps the cache.
   - Why the effects are at this level: an effect works on the placed picture of a clip, which can be partly clear.
   - A frame after the mix has no clear parts. Thus its codec needs no alpha.

One line shows both levels. A frame is blue when each slow part of it is in the cache.

## One record of each frame

Each kept layer has a `player::Origin` ([playback](../design/playback.md#the-engine)). It tells what made the layer, the upscaler, and when. The render cache uses these records. It adds no second record of how a frame is made.

- **The name of a frame:** the cache names a frame by its kept picture `Id` (`source_key` and the source position).
  - It adds only the size and the codec. At the timeline level, it also adds the placement and the mix.
  - A cached part keeps the `Id` of the frames that it replaces. Thus kept pictures, marks and Stats for Nerds name the same frame.
- **How it was made:** the cache keeps the `Origin` of each frame with the frame.
  - A frame from the cache shows that `Origin`, and one more fact: it came from the render cache, written at a given time.
  - The kept copy holds a date. Why: an `Instant` has no meaning in another process.
- **What it cost:** the `Origin` also holds the time that each layer took to decode and to enlarge.
  - It holds this time for a Retime Process and a Smooth Cut already.
  - The marks of dropped frames read these times.
- **Why a part renders:** one function of the plan gives the reasons of each part.
  - Smart, User, the line, `cache.status` and Stats for Nerds use it.
  - It names the processes as Stats for Nerds does (`interpolate::name`, `super_scale::name`).
- Stats for Nerds gets a Render Cache row in How It Was Made: Cached, the reason that it will render, or Not Needed.

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
- The mark covers the part of the plan that holds the stretch (`VideoPiece`), by the `Id` of its frames.
  - Thus a moved clip keeps its mark. An edit that changes the frames makes a new part without a mark.
- The marks are in the index of the cache, on this computer. Delete Render Cache > All removes them too.
- The numbers above are starting values. A measurement changes them, with a plan that the owner accepts first.

## Disk

- The cache is a new kind of the disk cache (`disk_cache::Kind`), in the cache dir.
  - It counts in the limit of the Cache page (`cache.percent`, 10% of the disk unless set).
  - The least recently used files go first.
- One cache serves each project. A part that two projects share renders once.
- Advanced has a folder for the cache, for example on a faster disk.
- Each file holds a few seconds of frames, with frequent keyframes. Its index holds the name and the `Origin` of each frame.
- Delete Render Cache:
  - All empties the cache.
  - Unused keeps only the frames of the open project.
  - Selected Clips removes the frames of the selected clips.

## Reading and writing

- **Read:** a new `apply_render_cache`, next to `apply_proxies`, puts the cached parts into the plan of the viewer.
  - Each cached part is a clip that reads a file of the cache.
  - Thus GPU decoding, kept pictures, steps back and backward play work as for any clip.
- **Write:** as `proxy::write`. The draw of the export makes the frames, and the GPU encoder writes a Matroska file (`disk_cache::added`).
- The background render uses the AI model that the viewer has loaded. It does not load a second copy.

## Settings and requests

- **The project:** the mode, the Render Cache of each clip, and the excluded tracks.
- **Preferences:** the Proxies page becomes Proxies and Render Cache, with these settings:
  - Background Render, on or off, and the time without activity before it starts
  - the kinds of User mode: Transitions, Composites and AI
  - under Advanced: Codec and Folder
- **Requests** for the CLI, IPC and MCP:
  - `project.set` with `render_cache`: the mode
  - `clip.set` with `render_cache`: auto, on or off
  - `track.set` with `render_cache`: false for an excluded track
  - `effects.set` with `render_cache`: true for a checked effect
  - `cache.status` (new): the parts to render and the parts in the cache, with their reasons, and the size on disk
  - `cache.delete` (new): all, unused, or the given clips
  - `cache.render` (new): renders the cache without a GUI, for scripts
  - `preferences.set` with the new keys

## Differences from Resolve

The owner accepted these. Each goes into [deviations.md](../deviations.md) with the pull request that makes it.

- Playback does not write the cache. The background render first writes the frames that playback made.
- With proxies on, the cache has the size and the codec of proxies.
- One cache serves all projects, under the limit of the disk cache. The least recently used files go first.
  - Resolve keeps a cache for each project, and deletes the files after a number of days.
- The settings of this computer are in Preferences, not in the project.
  - These are the background render and its time, the kinds of User mode, the codec and the folder.
- Smart also renders the parts where frames drop steadily.
- Smart also renders clips with AI Super Scale, and clips whose frames a Retime Process makes at another frame rate. The manual does not name these for Smart.
- Smart finds the media that are slow to decode by the proxy rules and by the decoder, not by a list of formats.
- These are added: the line in both timelines, the reasons under the pointer, and the Render Cache row of Stats for Nerds.

## Steps

1. **The clip level:**
   - the files and their index, the names of frames, the kept `Origin` and the reading
   - the background render, and Smart and User for the clip level
   - the mode, the requests and Preferences
2. **The GUI:** the Playback menu, the line with its reasons, Render Cache in the menu of a clip, and the row of Stats for Nerds.
3. **The timeline level:** transitions, opacity, composite modes, Render Cache Effects, and Exclude Track from Caching.
4. **Dropped frames:** the times of each layer in `Origin`, and the marks.

Each step adds its part to the spec, the design docs and the deviations. The last step deletes this file.
