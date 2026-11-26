# The crop, the thin-bar limit and the lean as measured

**When:** after batch PF of the measurements. The runbook is `picture-fit-pf-runbook.md` in the notes of the measurement session.

**What:** the measurements in Resolve Studio 21.2 show these differences in the placement stage. Each rests on one file or on few cases. Thus vidcut keeps its code until more cases agree.

- **The crop of pixels that are not square** (spec D75):
  - The one measured file is 1000 x 800 with pixels of 9:8.
  - In 1280 x 720, f = 0.9. Crop Left 3.0 cuts 3 source pixels in Resolve and 2 in vidcut.
  - In 1920 x 1080, f = 1.35. Crop Left 3.0 cuts 2 source pixels in Resolve and 1 in vidcut.
  - Both cuts are floor(crop / f), with f the factor of the fit without the pixel aspect ratio.
  - vidcut divides the crop by the fitted width, which has the pixel aspect ratio in it (`put_at` and `stored_value` in the GUI).
  - If more files agree, the crop of a side across divides by f × the source width.
- **The limit for a thin bar with a crop or Softness** ([spec](../spec.md#the-limit-for-a-thin-bar)):
  - In 1280 x 720, Rotation 0.016 with Crop Softness 0.5 shows no bar. vidcut shows the bars, because `Soft::of` gives a ramp.
  - Rotation 0.016 with Crop Left 0.5 shows the bars. vidcut shows no bar, because `whole_pixels` gives 0.
  - Rotation 0.010 with Crop Left 0.5 and Crop Softness 0.5 shows the bars. vidcut shows them too.
  - These cases are on one file, with one crop side and one value.
  - The change for Softness needs the change for the crop. Without it, the third case breaks.
- **The edge of a leaned picture** ([spec](../spec.md#the-limit-for-a-thin-bar)):
  - All Pitch and Yaw cases fit one model. Each edge that a lean moves is 0.5 px farther out than the projective lean of vidcut.
  - Pitch 0.0009 in 1280 x 720: vidcut puts the top edge 0.324 px down, and the top corners 0.576 px in. Resolve reads 0.925 at those corners.
  - Pitch 0.0011 in 1920 x 1080: vidcut puts the top edge 0.593 px down. Resolve reads 0.9067 at the top middle, a gap of 0.093 px.
  - With that model, gaps under 0.2 px show on a leaned picture. Thus the limit does not act there.
  - vidcut uses `no_bars` on the corners of the leaned picture. Without the 0.5 px, a change of the limit alone makes small leans worse.
  - Batch PF tells if the 0.5 px stays at Pitch 0.01 and 0.05.
- **A compound clip:** the outer level keeps the limit only when the clip inside has Zoom X 1. This is from 9 cases. Measure more cases for a general rule before a change.

The record is section M10 of `fixtures/program/NOTES.md` in the test data.
