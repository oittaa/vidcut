# Freeze frame and holds

**When:** next. Fourth in [the roadmap](../roadmap.md#next). A whole clip already freezes. What is missing is one chosen frame, and a hold at the head or the tail, in a form an OTIO file can carry.

`edit.change_speed` with `speed` 0 freezes the frame under the playhead, or the clip's first frame when the playhead is elsewhere, for the whole clip. It is written as a `FreezeFrame.1` with `time_scalar` 0, the sound is silent, and frames are not blended (spec D48, [OpenTimelineIO](../design/model.md#opentimelineio)). That request stays the whole-clip freeze.

## Holds

`edit.hold` writes a flat span of the same time map [speed ramps](speed-ramps.md) use: timeline time maps to a source time, and a hold is a stretch where the source time does not move.

| Form | What it writes |
|---|---|
| the whole clip | one source frame for every timeline frame; stored as `FreezeFrame.1`, as `speed` 0 already is |
| `head` | the clip grows by that many frames at its start; those frames show the clip's first source frame, then the clip plays |
| `tail` | the clip grows by that many frames at its end; the last source frame holds |
| `at` and `length` | the timeline span starting at `at`, `length` long, shows the one source frame under `at` |

`at` defaults to the playhead. `length` defaults to the rest of the clip. A head or a tail hold repeats a frame the media has; it does not invent frames past the media. The sound follows the source: a span whose source time is still is silent, as a freeze is today, and the sound of a head or a tail stops when the media's sound stops. Later clips move when ripple is on, and stay when it is off, as a speed change already does.

A constant slope is still `LinearTimeWarp.1`. A map that is flat everywhere is `FreezeFrame.1`. Anything else, including a hold in the middle of a clip that also plays, is the `TimeEffect.1` map. `edit.change_speed` on a clip that already has a varying map is refused; `speed.clear` (in [speed ramps](speed-ramps.md)) returns it to 1×.

Frame choice stays the rule D48 already states: each timeline frame shows the source frame its stretch starts in, with no blending. Blending and interpolation belong to speed ramps, and a hold does not ask for them.

## Requests

```bash
vidcut edit change-speed c3 --speed 0
vidcut edit hold c3 --at 12s
vidcut edit hold c3 --tail 2s
vidcut edit hold c3 --at 4s --length 1s
vidcut edit hold c3 --head 10f --ripple
```

```json
{"method":"edit.hold","params":{"clips":"c3","at":"12s","length":"3s"}}
```

MCP: `edit_hold`. One hold is one undo step. The window is a row on the clip: Hold Frame, Hold Head, Hold Tail, each a duration. Done sends one `edit.hold`.

## OpenTimelineIO

`FreezeFrame.1` and `LinearTimeWarp.1` are what vidcut already writes and reads for a constant speed. A varying map, a head hold and a tail hold are a `TimeEffect.1` whose `Key Frames` are the 7-element arrays Resolve writes: timeline seconds, source seconds, then five values. In the published example the first two are the times and the other five are `false, 0, 0, 0, 0` ([OpenTimelineIO discussion 1692](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/discussions/1692)). vidcut writes that shape. On a file it reads, the five are preserved. Their meaning is not documented, so vidcut does not interpret them.

The ease of a ramp is not in that array. It lives in the clip's `vidcut` metadata, as a transform's ease does. A hold has no ease.

Resolve's importer has not been checked with a `FreezeFrame` or a `TimeEffect` that vidcut wrote. The fixture is a file Resolve exported, plus one vidcut writes, and the round trip is confirmed by hand before the writer is called done. Titles were measured that way; this has not been.

## Tests

A `speed` of 0 still becomes `FreezeFrame.1` with `time_scalar` 0. A tail hold of 10 frames on a 100-frame clip maps timeline frames 0–99 through the source and frames 100–109 to the last source frame, and the clip's length is 110. A middle hold is flat in the map and is not a `FreezeFrame`. A synthesized `TimeEffect` key round-trips its seven values. Sound on a flat span is silent.

## Open questions

- Whether Resolve shows a `FreezeFrame` vidcut wrote, and a `TimeEffect` whose keys vidcut wrote, or only one it exported itself.
- Whether a head hold should reach before the media's start, the way a still's source can (spec D38). The request above repeats the first frame already in the clip and grows the clip at its head.
