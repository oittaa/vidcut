# Effects library

**When:** now, one pull request for each step below.

The Cut page has separate tabs for transitions, titles and effects (p. 636). vidcut follows it.

- The Transitions tab, the Transition Inspector, Dip To Color Dissolve, Smooth Cut, Push and Slide are there ([spec §8.11](../cut-page-spec.md#811-dissolves-pp-645-747-758761-895-1114-1159-1173)).
- The Effects tab, the effects of the Inspector and Gaussian Blur are there ([spec §8.16](../cut-page-spec.md#816-effects-pp-765-3526-3536)).
- The Generators page and Solid Color are there ([spec §8.17](../cut-page-spec.md#817-generators-p-766)).
- If the bar gets too full, one Effects tab with categories can replace the tabs, as the Edit page has (p. 802).

Each step is scriptable: the CLI, IPC and MCP can do each GUI action.

1. **More effects:** Mosaic, Sharpen, Vignette, Film Grain and Glow.
   - Measure each one in Resolve first: its values, their ranges and defaults, and its math.
2. **Generators** (p. 766), one pull request for each line:
   - Four Color Gradient.
   - The ramps and the SDR bars: 10 Step, 100mV Steps, Grey Scale, YCbCr Ramp, EBU Color Bar and SMPTE Color Bar.
   - The HDR bars and Window: the three BT.2111 Color Bars and Window.
3. **Later:**
   - Measure Smooth Cut in Resolve with a side that is a gap or a generator, a moved or resized side, and a side played backward. vidcut plays these as a Cross Dissolve now ([spec §8.11](../cut-page-spec.md#811-dissolves-pp-645-747-758761-895-1114-1159-1173)).
   - Measure the Gaussian Blur with Dynamic Zoom. vidcut does not change the sigma by the Dynamic Zoom box.
   - Measure where Resolve applies the effects of a compound clip ([todo](compound-opacity.md)).
   - Measure the Gaussian Blur with Use Alpha off on a clip that is partly clear, for example with Crop Softness. vidcut blurs its colors without their alpha.
   - Audio effects: equalizer, compressor, reverb and broadband noise reduction.
   - Voice Isolation separates a voice. Broadband noise reduction is the room and the hiss.
   - Keyframes of effect values.
   - Hover Scrub Preview of transitions, effects and generators (pp. 759, 765, 766).
   - Onscreen controls of effects in the viewer (p. 766).
   - Favorites (p. 760).
