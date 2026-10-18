# A GIF of one frame as a still

**When:** with the next work on stills or probing.

Resolve imports GIF and WebP files of one frame as stills (20.2 release notes). vidcut's probe takes as stills only what an image demuxer reads (image2, the `_pipe` ones), so a GIF of one frame comes in as a clip one frame long. Make it a still, as [D38](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions) does for other image files.
