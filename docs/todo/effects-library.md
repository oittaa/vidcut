# Effects library

**When:** now, one pull request for each step below.

The Cut page has separate tabs for transitions, titles and effects (p. 636). vidcut follows it.

- The Transitions tab, the Transition Inspector, Dip To Color Dissolve, Push and Slide are there ([spec §8.11](../cut-page-spec.md#811-dissolves-pp-645-747-758761-895-1114-1159-1173)). An Effects tab comes next.
- If the bar gets too full, one Effects tab with categories can replace the tabs, as the Edit page has (p. 802).

Each step is scriptable: the CLI, IPC and MCP can do each GUI action.

1. **The Transition Curve of Push and Slide** in the Inspector.
   - A Custom ease plays the keys of its curve. A request and an OTIO file can give the keys.
   - The Inspector has no curve editor yet. Thus a Custom ease set in the Inspector has no keys, and the transition stays on the outgoing picture, as in Resolve.
2. **Smooth Cut** (p. 759). It hides a jump cut with the frame interpolation of Retime Process.
3. **Effects tab and clip effects** (p. 765).
   - A drag onto a clip adds the effect. A double-click adds it to the selected clip.
   - The Inspector gets an Effects panel with the values of each effect.
   - The effects come after Transform and Crop, and before compositing ([spec §13](../cut-page-spec.md#13-order-of-operations-pp-287296-3407)).
   - First effects: Gaussian Blur, Mosaic, Sharpen, Vignette, Film Grain and Glow.
4. **Generators** (p. 766): Solid Color, Color Bars and Gradient.
5. **Later:**
   - Audio effects: equalizer, compressor and reverb.
   - Keyframes of effect values.
   - Hover Scrub Preview of transitions and effects (pp. 759, 765).
   - Onscreen controls of effects in the viewer (p. 766).
   - Favorites (p. 760).
