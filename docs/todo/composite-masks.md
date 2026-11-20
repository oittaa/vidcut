# Composite: titles between layers

**When:** not decided.

vidcut has the composite modes from Normal to Inverted Lum ([spec D75](../spec.md#12-decisions-on-gaps-and-contradictions)). This part is open.

- **A title between layers, checked by hand:**
  - Measured in Resolve Studio 21.1.2: V2 is at Add over V1, V3 has a title, and V4 is 50% gray at Multiply.
  - Outside the title, the frame is (V1 + V2) × 0.5. The sum is not cut at 1.
  - The viewer and the exports draw the title into the floats of the layers below it. Compare an export with these values.
