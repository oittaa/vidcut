# Title nodes in compositions

**When:** not decided.

The Text+ tool and the Merge tool of the compositions get the rest of their controls ([spec §8.19](../spec.md#the-text-tool)).

**Why:**

- Resolve's Text+ title and its Fusion titles are Fusion compositions with text tools (p. 1178).
- The Text+ tool and the Merge tool have more controls than vidcut has now (pp. 2217–2219, 2383–2393).

**What Resolve has:**

- The Text+ node has these controls that vidcut does not have yet:
  - These controls of the shading elements (pp. 2389–2393):
    - Sort By Z.
    - Blending: Composite, Solid or Transparent where two characters overlap.
    - Line Style: dashes and dots. Adapt Thickness to Perspective.
    - Color Types Gradient and Image.
    - Softness Glow, Softness Blend and Apply Softness to Fill Color.
    - Offset Z, and the Pivot, Rotation, Shear and Size of each element.
  - The Frame, Circle and Path layouts, with wrap and justify (pp. 2384, 2387).
  - The Advanced Controls of the Text tab: kerning, ligatures and font features (p. 2386).
  - The Transform tab (p. 2388).
  - Character Level Styling and the text modifiers (p. 2383).
- In the viewer, a Text+ has its text to edit, and handles for its Center.
- The Merge node has these controls that vidcut does not have yet:
  - Filter Method: Linear is the only filter in vidcut. Resolve has 11 filters (p. 2217).
  - Window Method, for the Sinc and the Bessel filters (p. 2217).
  - Reference Size (p. 2218).
  - Flatten Transform (p. 2218).
  - Clamp Coverage, under Blend. It is on in a new Merge of Resolve Studio 21.2. The manual does not describe it.

**To measure in Resolve:**

- Dissolve at a Blend below 1.
- Subtractive/Additive, Alpha Gain and Burn In with an Apply Mode other than Normal.
  - With Saturation, the Inspector shows only Gain and Blend. The Inspector of vidcut shows all three sliders with each mode.
- Invert Transform, also with a flip.
