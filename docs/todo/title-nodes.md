# Title nodes in compositions

**When:** not decided.

The compositions of vidcut get a Text+ tool ([spec §8.19](../spec.md#819-compositions-pp-1332-1385-1403-1420-21082111-2472-2480-2483-24902492-2500)). The Merge tool has the rest of its controls still to do.

**Why:**

- Resolve's Text+ title and its Fusion titles are Fusion compositions with text tools (p. 1178).
- A Merge has more controls than vidcut has now (pp. 2216–2218).

**What Resolve has:**

- The Text+ node draws text into a 2D picture. Its one input is an effect mask that crops the text (p. 2381).
- The Merge node has these controls that vidcut does not have yet:
  - Filter Method: Linear is the only filter in vidcut. Resolve has 11 filters (p. 2216).
  - Window Method, for the Sinc and the Bessel filters (p. 2216).
  - Reference Size (p. 2217).
  - Flatten Transform (p. 2217).
  - The Channels tab and the Depth Merge (p. 2218).
- In the viewer, a Merge has handles for its Center, Size and Angle.

**To measure in Resolve:**

- Dissolve at a Blend below 1.
- Subtractive/Additive, Alpha Gain and Burn In with an Apply Mode other than Normal.
- Invert Transform, also with a flip.
