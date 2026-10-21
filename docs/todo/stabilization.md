# Stabilization

**When:** later. Last in [the roadmap](../roadmap.md#later). The smoothed motion is a per-frame [clip transform](clip-transform.md), so it waits on that. The vid.stab releases distributions ship are GPL, so they are not the default in an MIT build.

Resolve's stabilizer is an analysis and a transform on the clip, with a crop or a zoom that hides the missing edges. vidcut does the same as two requests, the way scene detection is a measurement and then an edit ([analysis](../design/analysis.md)).

## Own analysis

`analyze.stabilize` measures. `clip.stabilize` applies. The measurement is cached by the file's origin, like picture stats. The analysis runs on the CPU. Applying it is a picture placement, so it uses whatever path [clip transform](clip-transform.md#drawing) uses for a per-frame position, zoom and rotation.

The default method is vidcut's. It estimates a similarity per frame: translation, rotation and zoom. The camera path is then smoothed. The smoother is the L1 path of Grundmann, Kwatra and Essa, "Auto-Directed Video Stabilization with Robust L1 Optimal Camera Paths", CVPR 2011, which is the path vid.stab is built on, implemented here. A Gaussian smooth is the fallback the same paper's predecessors use, selected with `smooth: gaussian` when someone wants the cheaper curve. Both are determined by the motion and the settings; a test with a synthetic pan locks the path.

`crop: fill` (the default) zooms the smallest amount that keeps the empty border out of the frame for the whole clip, and writes that zoom into the picture. `crop: none` leaves the border, which is black or a lower track, as a placed picture already composites. `amount` from 0 to 1 is how hard the path is smoothed; 0 leaves the camera as it was.

The written result is `picture` keys, one position, rotation and zoom per frame that needs it, so it round-trips once clip transform's OTIO does. The analysis settings (method, amount, crop, the hash of the motion) go in the clip's `vidcut` metadata. Resolve will not re-run the analysis. It can show the position and zoom if it applies the Transform those keys become.

A clip that already has picture keys is refused until `clip.stabilize` is given `overwrite`, so a Ken Burns is not replaced by a silence. `stabilize.clear` removes the keys it wrote and leaves keys the user set.

## vid.stab

FFmpeg's `vidstabdetect` and `vidstabtransform` exist when the build was configured with `--enable-libvidstab` ([ffmpeg-filters, vidstabdetect](https://ffmpeg.org/ffmpeg-filters.html#vidstabdetect)). They call libvidstab.

Tagged releases through v1.1.2 are GPL-2.0-or-later. Master, not tagged as a release, relicensed `src/` to LGPL-2.1-or-later; the repository's `LICENSE`, `RELICENSE.md` and the 1.3 changelog say so, and the permission is in [issue 165](https://github.com/georgmartius/vid.stab/issues/165). A distro FFmpeg that enables the filters links the GPL release, and that build of FFmpeg is GPL as a whole.

`method: vidstab` is accepted only when `avfilter_get_by_name("vidstabdetect")` finds the filter. The answer says the filter is present and that the usual library it links is GPL. It is never the default, and vidcut does not vendor the library. The transforms it returns are converted into the same `picture` keys, so the rest of the pipeline does not grow a second placement. A build without the filter refuses `vidstab` and names the default method.

## Rolling shutter

A per-frame similarity cannot unwarp a frame that jelly-wobbles: that needs a warp per line, from a gyro or from a line model. vidcut does not read a gyro stream. A line warp that is wrong is worse than the wobble. Rolling shutter is out of the first version. When a gyro track exists on a file vidcut can read, a later version can feed it; a phone's gyro is not assumed to be that track.

## Requests

```bash
vidcut analyze stabilize c3 --amount 0.6
vidcut clip stabilize c3 --amount 0.6 --crop fill
vidcut clip stabilize c3 --method vidstab --amount 0.4
vidcut stabilize clear c3
```

```json
{"method":"clip.stabilize","params":{"clips":"c3","amount":0.6,"crop":"fill"}}
```

MCP: `analyze_stabilize`, `clip_stabilize`, `stabilize_clear`. The analysis is a job. The apply is one undo step of `picture` keys. The window is Stabilize on the clip: amount, fill, and Advanced for the method and the smoother. The line under it says when the filter is absent, or when the linked stabilizer is GPL and was not asked for.

## Tests

A synthetic translate of a known number of pixels per frame, with `amount` 1, becomes a path a test writes out (the L1 path on that input). `crop: fill` on a path whose worst border is 5% writes a zoom the test names. `method: vidstab` with no filter is an error. A clip with a user zoom and no `overwrite` is refused. Identity motion writes no `picture`.

## Open questions

- The L1 solver's size on a long clip. The first version smooths a window if a whole-clip solve is too slow, and the window length is a constant the test names.
- A tagged LGPL vid.stab. Until one is what FFmpeg links, `vidstab` stays optional and named as GPL in the answer.
- Rolling shutter, only with a gyro track vidcut actually parses.
