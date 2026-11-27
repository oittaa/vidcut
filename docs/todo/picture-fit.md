# The anchor point in a turn, and compound clips as measured

**When:** next.

**What:** the measurements in Resolve Studio 21.2 show these differences in the placement stage. Each difference is a fault in that stage (AGENTS.md rule 1).

- **The anchor point in a turn on pixels that are not square** ([spec D75](../spec.md#12-decisions-on-gaps-and-contradictions)):
  - Resolve moves the point at the anchor in a Rotation. On the 2:1 file at Rotation 30, it moves 63.2 px right and 40.5 px up.
  - The 4:3 file shows the same model. On square pixels, the point stays.
  - vidcut keeps that point in its place, as a turn about the anchor point does on square pixels.
  - The owner decides: copy this, or record it as a fault of Resolve in gotchas.md.
- **A compound clip:** the outer level keeps the limit only when the clip inside has Zoom X 1. This is from 9 cases. Measure more cases for a general rule before a change.

The records are sections M10, PG and PH of `fixtures/program/NOTES.md` in the test data.
