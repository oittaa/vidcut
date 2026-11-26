# The lean and compound clips as measured

**When:** next.

**What:** the measurements in Resolve Studio 21.2 show these differences in the placement stage. Each difference is a fault in that stage (AGENTS.md rule 1).

- **The edge of a leaned picture** ([spec](../spec.md#the-limit-for-a-thin-bar)):
  - Resolve opens a gap 0.5 px less than the projective lean of vidcut, and not less than 0. Other bars add to that gap.
  - Pitch 0.0009 in 1280 x 720: vidcut puts the top corners 0.576 px in. Resolve reads 0.925 there, a gap of 0.076 px.
  - Pitch 0.01 and 0.05: vidcut puts the top edge at y = 3.564 and 17.143. Resolve puts it at 3.064 and 16.643.
  - A lean also stops the limit: Zoom X 0.9997 with Pitch or Yaw 0.0003 shows bars of 0.192.
  - The two changes go together. A change of the limit alone opens gaps under 0.5 px at small leans, where Resolve shows none.
  - Not known: how the 0.5 px works with a Rotation or a crop. Find the cause before a change.
- **A compound clip:** the outer level keeps the limit only when the clip inside has Zoom X 1. This is from 9 cases. Measure more cases for a general rule before a change.

The records are sections M10 and PF of `fixtures/program/NOTES.md` in the test data.
