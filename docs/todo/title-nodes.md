# Title nodes in compositions

**When:** not decided.

The Text+ tool and the Merge tool of the compositions get the rest of their controls ([spec §8.19](../spec.md#the-text-tool)).

**Why:**

- Resolve's Text+ title and its Fusion titles are Fusion compositions with text tools (p. 1178).
- The Text+ tool and the Merge tool have more controls than vidcut has now (pp. 2216–2218, 2383–2392).

**What Resolve has:**

- The Text+ node has these controls that vidcut does not have yet:
  - The shading elements 2 to 8: outlines, shadows and borders (pp. 2389–2392).
  - The Frame, Circle and Path layouts, with wrap and justify (pp. 2384, 2387).
  - The Advanced Controls of the Text tab: kerning, ligatures and font features (p. 2386).
  - The Transform tab (p. 2388).
  - Character Level Styling and the text modifiers (p. 2383).
- In the viewer, a Text+ has its text to edit, and handles for its Center.
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
