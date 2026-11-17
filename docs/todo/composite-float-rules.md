# Composite modes by the float rules

**When:** next.

Six rules of the composite modes come from 16-bit renders of Resolve. Float renders of Resolve show other rules ([D75](../spec.md#12-decisions-on-gaps-and-contradictions), [X171](../deviations.md#x171-six-composite-rules-come-from-16-bit-renders)).

**Why:**

- A value above 1 or below 0 stays between the layers ([spec §13](../spec.md#13-order-of-operations-pp-287296-3407)). A cap cuts these values.
- An OTIO file then gives the same picture in vidcut and in Resolve.

**In the code:**

- `dodge` and `burn` in `crates/vidcut-av/src/screen/picture.wgsl` clamp the results of `dodged` and `burned`.
- `divide` clamps `b / a` and has a special case for `a ≤ 0`.
- `mixed` gives `a + b − 1` for Linear Burn, takes `min(b, 2a)` for Pin Light at `a = 0.5`, and takes `b` on a tie of Lighter Color and Darker Color.

**To do:**

- Remove the cap of Color Dodge and the floor of Color Burn. Divide becomes `b / (a + 0.000001)`.
- Linear Burn stops at 0. Pin Light takes the max branch at `a = 0.5`. Lighter Color and Darker Color take the clip on a tie.
- The doc comments of `CompositeMode` give the new formulas.
- The doc comment of `composite_type` lists only the rule of Hue with a gray Color.
- Remove X171.
