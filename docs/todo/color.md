# Basic color

**When:** later. Fifth in [the roadmap](../roadmap.md#later). Exposure, contrast, white balance, tint, saturation and a LUT finish a picture here. A grade does not survive into Resolve, which is what use case 1 is for, so this waits until the picture and the sound can be finished.

Resolve's grades live on the Color page and in a `.drx`, not in the OTIO the Cut page exchanges. Published OTIO clips carry no color effect. ResolveFX do not come across ([X9](../deviations.md#x9-opentimelineio-interchange)). vidcut stores the grade in the project and applies it on playback and on a media export. An OTIO export keeps the cut and warns.

## Grade

One grade per clip, `color`, omitted when it is the identity. It runs in the scene-linear light the export already uses, and the HDR tone map already in the path runs after it (spec D35). It does not grade in display-encoded 8-bit.

| Field | Identity | Meaning |
|---|---|---|
| `exposure` | `0` | stops, added in linear light |
| `contrast` | `1` | about mid grey |
| `temperature` | `6504` | kelvin; the RGB gains of a Planckian locus, the formula locked by a test |
| `tint` | `0` | green toward magenta, −1 to 1 |
| `saturation` | `1` | in the linear light, after the white balance |
| `lut` | none | a `.cube` applied after the numbers |

`exposure` from −8 to 8, `contrast` from 0 to 2, `temperature` from 2000 to 11000, `saturation` from 0 to 2. The user's numbers are what run. A value the control cannot represent is an error, not a clamped substitute.

A LUT is a 3D `.cube` of size 17, 33 or 65. The CPU path interpolates tetrahedrally. The GPU path is the viewer shader, and on CUDA the libplacebo already in that color path. Another GPU comes down to the CPU grade while a grade shows, the same gap a title has. An identity grade takes the fast path and samples nothing.

The grade is on footage, stills, sequences and a compound treated as one picture. A title is not graded. Two clips with grades dissolve after each is graded.

## Automatic balance

`color.balance` reads the frame under `at` (the playhead when `at` is omitted) and writes `temperature` and `tint`. Grey-world is the default: the frame's mean RGB in linear light becomes neutral. `method: highlight` uses a white patch, the brightest few percent, and refuses a frame with no highlight. The result is the same fields a person would type, so it undoes as one `color.set`. It does not keep a separate "auto" flag.

`color.match` matches `clips` to `reference`, a clip. It matches each channel's mean and variance in linear light, and writes those into `exposure`, `contrast` and `tint` (the temperature stays, unless `white_balance` is set, in which case the reference's balance is copied first). The statistic is the default because it is determined by the frames and it runs on a laptop. A learned matcher is added only if a small MIT or Apache-2.0 model beats this statistic on a fixture in `$VIDCUT_TEST_DATA`, and it is off unless `method` names it. No model ships in the first version.

## Requests

```bash
vidcut color set c3 --exposure 0.5 --temperature 5200 --saturation 1.1
vidcut color set c3 --lut looks/print.cube
vidcut color balance c3
vidcut color match c4 --reference c3
vidcut color get c3 --json
vidcut color clear c3
```

```json
{"method":"color.set","params":{"clips":"c3","exposure":0.5,"temperature":5200,"tint":0.05}}
```

MCP: `color_set`, `color_get`, `color_clear`, `color_balance`, `color_match`. The Color window is labeled rows for the five numbers, a LUT row, and Balance and Match. Done is one request. Dragging exposure previews and sends on release.

## Interchange

The grade is in the `.vidcut` file. On OTIO export it is also written in the clip's `vidcut` metadata. Where the five numbers fit an ASC CDL (slope, offset, power, saturation), the export also writes an `ASC_CDL` effect so another OTIO reader can see them; a LUT and a temperature that does not fit are metadata only. The export warns, naming every clip that has a grade, that Resolve will not apply it. The picture is not baked into new media on that export: baking would replace the cut. A media export (a movie, a still) applies the grade, because the viewer and that export match.

Import reads a `vidcut` grade back. An `ASC_CDL` with no vidcut grade becomes `color` as far as the five numbers go, and the answer says the LUT was not in the file. A Resolve grade that arrived as an unknown effect stays in `Kept` and is not played.

## Tests

The Planckian gains at 6504 K are the identity, the three channels written out. Exposure of +1 doubles linear light. A 2-cube LUT with the eight corners written out interpolates the center to the expected RGB. Grey-world on a frame of known means writes the temperature and tint the test names. Identity is absent from the project JSON and from the plan. A CDL fixture round-trips slope and saturation.

## Open questions

- The CDL slope for a temperature that is not a pure RGB gain. The first version writes the RGB gains as slope and says so.
- A learned matcher. None until it beats the statistic on a named fixture.
- Whether a LUT may be a `.cube` larger than 65. The first version refuses it.
