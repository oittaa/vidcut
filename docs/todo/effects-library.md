# Effects library

**When:** now, one pull request for each step below.

The Cut page has separate tabs for transitions, titles and effects (p. 636). vidcut follows it.

- The Transitions tab, the Transition Inspector, Dip To Color Dissolve, Smooth Cut, Push and Slide are there ([spec §8.11](../cut-page-spec.md#811-dissolves-pp-645-747-758761-895-1114-1159-1173)). An Effects tab comes next.
- If the bar gets too full, one Effects tab with categories can replace the tabs, as the Edit page has (p. 802).

Each step is scriptable: the CLI, IPC and MCP can do each GUI action.

1. **Effects tab and clip effects** (p. 765).
   - A drag onto a clip adds the effect. A double-click adds it to the selected clip.
   - The Inspector gets an Effects panel with the values of each effect.
   - The effects come after Transform and Crop, and before compositing ([spec §13](../cut-page-spec.md#13-order-of-operations-pp-287296-3407)).
   - First effects: Gaussian Blur, Mosaic, Sharpen, Vignette, Film Grain and Glow.
2. **Generators** (p. 766): Solid Color, Color Bars and Gradient.
3. **Later:**
   - Measure Smooth Cut in Resolve with a side that is a gap, a moved or resized side, and a side played backward. vidcut plays these as a Cross Dissolve now ([spec §8.11](../cut-page-spec.md#811-dissolves-pp-645-747-758761-895-1114-1159-1173)).
   - Audio effects: equalizer, compressor and reverb.
   - Keyframes of effect values.
   - Hover Scrub Preview of transitions and effects (pp. 759, 765).
   - Onscreen controls of effects in the viewer (p. 766).
   - Favorites (p. 760).
