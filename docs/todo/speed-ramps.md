# Speed ramps

**When:** later. Sixth in [the roadmap](../roadmap.md#later). A speed that changes along a clip reuses the key curves from [clip transform](clip-transform.md#keyframes). Constant speed already exists (spec D48), and anything finer than the nearest frame stays off a laptop preview.

Resolve writes a variable retime as `TimeEffect.1`. The Cut page's Change Clip Speed is the constant case vidcut already has, including a freeze at speed 0 ([freeze frame](freeze-frame.md)).

## The map

The stored value is source time against timeline time, not a speed. A hold is then a flat span, which is what [freeze frame](freeze-frame.md) writes, and it is the shape of Resolve's keys. Speed at a frame is the slope.

A key is a timeline time on the clip (`at`), the source time it shows (`source`), and the ease of the segment after it. The eases are the five in [clip transform](clip-transform.md#keyframes): `linear`, `hold`, `ease_in`, `ease_out`, `ease`. `hold` keeps the source time until the next key, which is a freeze. The map is monotonic in source time for a forward clip, and monotonic backward for a reversed one; a key that turns it around is an error. Whole frames, as everywhere else.

A constant slope is stored as `LinearTimeWarp.1` with that `time_scalar`, and a map that is one source frame everywhere as `FreezeFrame.1`. Both are what D48 writes today. A varying map is `TimeEffect.1`. `edit.change_speed` remains the constant control. On a clip whose map varies it is refused.

`frames` chooses how a timeline frame is filled when the source time is between frames:

| `frames` | Does |
|---|---|
| `nearest` | the source frame the stretch starts in, which is D48; the default |
| `blend` | the two source frames mixed by the fraction, with the dissolve mixer |
| `flow` | optical flow, when this FFmpeg has it |
| `interpolate` | a learned interpolator, an export job |

`nearest` and `blend` run on the CPU and, when `gpu.acceleration` is on and the GPU matches, on the GPU. `blend` uses the same mix as a dissolve. They are cheap enough for the viewer. `flow` and `interpolate` are export-only; the viewer shows `nearest` and the export's report says so.

`flow` calls FFmpeg's `minterpolate` through `libavfilter` when `avfilter_get_by_name` finds it ([ffmpeg-filters, minterpolate](https://ffmpeg.org/ffmpeg-filters.html#minterpolate)). It is an ordinary filter, not one of the GPL filters. A build without it refuses `flow` and names `nearest` and `blend`.

`interpolate` is RIFE-class. The runner is [nihui/rife-ncnn-vulkan](https://github.com/nihui/rife-ncnn-vulkan) (MIT). The paper is [arXiv:2011.06294](https://arxiv.org/abs/2011.06294). Weights are a pinned file with a SHA-256, fetched with `download` like a speech model, and only after that file's own license is MIT or Apache-2.0. The Practical-RIFE README says the weights it links are MIT ([hzwer/Practical-RIFE](https://github.com/hzwer/Practical-RIFE)); the pin records the URL, the hash and the license text that was true for that file. Vulkan. With `gpu.acceleration` off, `interpolate` is refused and the answer offers `nearest` and `blend`. It is never the preview.

Sound keeps its pitch at any constant speed, as it does today ([X30](../deviations.md#x30-slowed-sound-keeps-its-pitch-on-every-system)). Along a ramp the pitch stays too: the map retimes the samples and does not resample them as a tape would. A flat span is silent (D48).

## Requests

```bash
vidcut speed set c3 --keys '[{"at":"0","source":"0","ease":"ease"},{"at":"2s","source":"2s","ease":"linear"},{"at":"3s","source":"2s"},{"at":"5s","source":"6s"}]'
vidcut speed set c3 --frames blend
vidcut speed get c3 --json
vidcut speed clear c3
vidcut edit change-speed c3 --speed 0.5
```

```json
{"method":"speed.set","params":{"clips":"c3","frames":"nearest","keys":[{"at":"0","source":"0","ease":"ease"},{"at":"4s","source":"8s"}]}}
```

The keys above play the first two seconds at 1×, hold the frame at two seconds for one second, then play to six seconds of source over the last two seconds. MCP: `speed_set`, `speed_get`, `speed_clear`. `speed.clear` is 1× and `nearest`.

The window shows the map for the selected clip: keys on a line, an ease per segment, and the frame mode. Dragging a key previews `nearest` and sends `speed.set` on release. A title has no speed, as today.

## OpenTimelineIO

`TimeEffect.1` `Key Frames` are arrays of seven values. In Resolve's published export the first is timeline seconds, the second is source seconds, and the other five are `false, 0, 0, 0, 0` ([OpenTimelineIO discussion 1692](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/discussions/1692)). vidcut writes that, with the five preserved when they were read. The ease and `frames` go in the clip's `vidcut` metadata, because the array does not hold them. A constant scalar stays `LinearTimeWarp.1` or `FreezeFrame.1` and is not also a `TimeEffect`.

`Speed Ratio` on a `TimeEffect` is the overall ratio Resolve wrote beside the keys. vidcut writes the ratio of the source span to the timeline span and, on import, trusts the keys.

A ramp vidcut wrote has not been checked in Resolve's importer. The fixture is one Resolve exported and one vidcut writes, compared by hand.

## Tests

The map above: timeline frame at 2.5 s of a 25 fps clip shows the source frame at 2 s. `hold` between two keys does not advance the source. A key that decreases source time on a forward clip is refused. `ease` at the middle of a segment is the smoothstep value written out. A constant `speed.set` of 2 becomes `LinearTimeWarp.1` with `time_scalar` 2. A `TimeEffect` fixture keeps all seven numbers. `frames: flow` when the filter is absent is an error that names the modes that exist.

## Open questions

- The five undocumented `TimeEffect` numbers. Preserved, and written as `false, 0, 0, 0, 0` when vidcut creates the key, until one of them is shown to be an ease or a tangent.
- Which Practical-RIFE (or other RIFE) weight file is pinned, after its license file is read. None is pinned in this spec.
- Audio on a ramp that also reverses. The first version allows a reverse only as a constant negative `edit.change_speed`, which already exists.
