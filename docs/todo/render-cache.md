# Render cache

**When:** not decided.

Some frames take longer to make than the viewer shows them. The viewer then cannot play them in real time. These frames are such:

- **Retime Process with an AI process:** a 1080p clip plays at 25% on an M4 with macOS 27. The viewer shows 48% of its frames in time. In place of the others, it shows the frame before.
- **Super Scale with an AI upscaler:** on the same Mac, AI of Super Scale takes 474 ms for a frame of 540p shown at 1080p.
- **Many layers:** dissolves, clips over each other with an opacity, and effects that vidcut adds later.

The player keeps the frames that it made in memory, while its budget lasts. Thus a loop over a short part of the timeline fills in more frames each time. A longer part does not fit.

Resolve has a Render Cache (pp. 200–204):

- In its Smart mode, it renders the clips with slow effects in the background, into files on disk. Playback then reads these files in place of the effects.
- It caches speed effects at the level of the source. Thus a clip keeps its cache when it moves on the timeline.

A cache of vidcut could work in this way:

- While the player is idle, a background job makes the slow frames and writes them to disk.
- Each frame has the key of all that made it: the source frames, the process, the size and the settings. A change of a setting then misses the cache, and old frames stay valid for an undo.
- Playback reads a cached frame in place of making it.
- A setting gives the folder and the most disk space. The oldest frames go first.
