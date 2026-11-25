# The fit, the crop and the thin-bar limit as measured

**When:** next.

**What:** the measurements in Resolve Studio 21.2 show these differences in the placement stage. Each difference is a fault in that stage (AGENTS.md rule 1).

- **The fit of pixels that are not square, or of a file with a rotation** ([spec](../spec.md#the-fit-to-the-timeline)):
  - Resolve fits the display size or the upright size with no rounding. A 9:8 file of 1000 x 800 is 1012.5 x 720 in 1280 x 720.
  - `fitted` sends such a file to `rounded_place`. That rounds the size to even and the place to whole pixels.
  - The fit must not branch on the kind of the file.
- **The crop of pixels that are not square** (spec D75):
  - Resolve cuts floor(crop / f) source pixels. f is the factor of the fit, without the pixel aspect ratio.
  - vidcut divides the crop by the fitted width, which has the pixel aspect ratio in it.
  - Thus Crop Left 3.0 on the 9:8 file in 1280 x 720 cuts 3 source pixels in Resolve and 2 in vidcut.
- **The limit for a thin bar** ([spec](../spec.md#the-limit-for-a-thin-bar)):
  - Crop Softness alone keeps the limit in Resolve. vidcut drops the limit when `Soft::of` gives a ramp.
  - A crop that cuts no pixel stops the limit in Resolve. vidcut keeps the limit, because `whole_pixels` gives 0.
  - The limit does not act on Pitch and Yaw in Resolve. vidcut uses `no_bars` on the corners of the leaned picture.
  - A compound clip keeps the limit of the outer level only when the clip inside has Zoom X 1. This is from 9 cases. Measure more cases for a general rule before a change.

The record is section M10 of `fixtures/program/NOTES.md` in the test data.
