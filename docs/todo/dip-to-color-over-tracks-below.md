# A dip to color over the tracks below

**When:** not decided.

**Why:**

- The [spec](../cut-page-spec.md#811-dissolves-pp-645-747-758761-895-1114-1159-1173) gives the measured rules, under A Dip To Color Dissolve.
- The dip arm of `plan_lanes` (`render.rs`) makes a new stack for each frame of the dip.
- That stack is the color on the whole frame, and the fading clip over it.
- The layers of the tracks below are not in that stack.
- Thus the plan agrees with the rules only for a clip that fills the frame, at full opacity, with the mode Normal.
- The plan differs from the rules in these cases:
  - Around a clip that does not fill the frame, the color is at full level on each frame of the dip. The tracks below do not fade.
  - A clip with an opacity below 100 mixes with the color. The tracks below do not show under it.
  - A clip with a composite mode has only the color below it.
  - A compound clip with the dip inside it gets the same stack. Thus the color covers the tracks below around it and under it.
  - With no tracks below, the area around such a clip changes from black to the color in one frame.

**How:**

- The dip takes the route of a transition with two sides ([design/export.md](../design/export.md#placed-pictures), `crossing`).
- The color is the other side of each half. It fills the frame, and its level is 1 − the level of the clip.
- The mode of the outgoing clip mixes both halves with the tracks below. With no outgoing clip, the mode of the incoming clip does.
- With a clip on one side only, the other half has the color and the tracks below.
- The layers of the tracks below stay in the plan.
- Inside a compound clip, the color is a part of the picture of the compound clip. It does not cover the tracks below the compound clip.

**To do:**

- Change the plan. Then compare frames of an export with Resolve's frames, for each case above.
