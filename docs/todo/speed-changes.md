# Speed changes

**When:** later. Clips at a speed play and edit, and Change Clip Speed sets it (spec D48).

- **Analyses:** Ripple Delete Silence and Detect Scene Cuts look at clips at another speed, their source mapped to the timeline through it (`analysis::Span`).
- **Compound clips:** one with a speed change plays the clips it holds at it (`render::expanded`), backward in reverse order, and decomposes.
