# Effects library

**When:** now, one pull request for each step below.

The Cut page has separate tabs for transitions, titles and effects (p. 636). vidcut follows it.

- The Transitions tab, the Transition Inspector, Dip To Color Dissolve, Smooth Cut, Push and Slide are there ([spec §8.11](../cut-page-spec.md#811-dissolves-pp-645-747-758761-895-1114-1159-1173)).
- The Effects tab, the effects of the Inspector, Gaussian Blur, Mosaic Blur, Glow, Film Grain and Vignette are there ([spec §8.16](../cut-page-spec.md#816-effects-pp-765-3526-3536)).
- The Generators page and its twelve kinds are there ([spec §8.17](../cut-page-spec.md#817-generators-p-766)).
- If the bar gets too full, one Effects tab with categories can replace the tabs, as the Edit page has (p. 802).

Each step is scriptable: the CLI, IPC and MCP can do each GUI action.

1. **More effects:** Sharpen.
   - Measure each one in Resolve first: its values, their ranges and defaults, and its math.
   - **Glow, measure these.** vidcut's choices for them are in the [spec](../cut-page-spec.md#816-effects-pp-765-3526-3536):
     - Opacity below 1 on a picture with alpha, on the Edit page.
     - The size of the Atmosphere noise on other timeline sizes, with Threshold 0.25 and Spread 0 on a gray picture.
     - The exact sigma between 580.8 and 633.6 pixels that moves the mirror to the grid of 16.
     - A Zoom with Secondary Glow.
   - **Film Grain, measure these.** vidcut's choices for them are in the [spec](../cut-page-spec.md#816-effects-pp-765-3526-3536):
     - What a film stock does to the other values, and when Film Grain Presets shows Custom.
     - Detail Loss on a square frame, for example 1440×1440. Does its sigma follow the area or the short side?
     - Grain Only at Grain Size 0, 0.1 and 0.25, with Softness 0, on 3840×2160 and 1280×720. On 3840×2160, vidcut gives a small grain 0.61 to 0.75 of the render's standard deviation.
     - Symmetry at Grain Strength 0.25 to 1, below 0.3 and above 0.9.
     - Red, Green and Blue with a Symmetry other than 0.5.
     - Texture under 0.25 and Grain Aspect Ratio under 0.5.
   - **Vignette, measure these.** vidcut's choices for them are in the [spec](../cut-page-spec.md#816-effects-pp-765-3526-3536):
     - Blend on the Edit page, at 0.5.
     - A Pan in another frame shape: a 1920×1080 source in a 1080×1920 timeline, Pan Y 800.
     - Hue at grays other than 0, 0.2, 0.4, 0.6, 0.8 and 1, for example 0.3 and 0.5.
     - An odd width or height of the picture.
   - **Mosaic Blur, measure these on the Edit page.** vidcut's choices for them are in the [spec](../cut-page-spec.md#816-effects-pp-765-3526-3536):
     - Blur Alpha off and Whole Cells Only.
     - Whole Cells Only with Hexagon, with Triangle, with other Aliasing values and with Antialias Result. The alpha limit of a whole cell.
     - Pixel Frequency 25 and 10 with Zoom, with a Pan and on other timeline sizes.
     - Position away from the middle with a Zoom or a Pan. A rotated clip, a crop, a Dynamic Zoom box and a compound clip.
     - A speed change or a render cache frame with Blur Alpha off or Whole Cells Only. vidcut keeps no color at an alpha of 0 there.
2. **Generators** (p. 766), one pull request for each line:
   - Measure on a generator: Dynamic Zoom, Stabilization, keyframes in a render, Push, Slide, Wipe and Dip To Color. vidcut plays them as on a still.
3. **Later:**
   - Measure Smooth Cut in Resolve with a side that is a gap, a moved or resized side, and a side played backward. vidcut plays these as a Cross Dissolve now ([spec §8.11](../cut-page-spec.md#811-dissolves-pp-645-747-758761-895-1114-1159-1173)).
   - Measure the Gaussian Blur with Dynamic Zoom. vidcut does not change the sigma by the Dynamic Zoom box.
   - Measure effects other than Gaussian Blur on a compound clip ([todo](compound-opacity.md)).
   - Measure the Gaussian Blur with Use Alpha off on a clip that is partly clear, for example with Crop Softness. vidcut blurs its colors without their alpha.
   - Audio effects: equalizer, compressor, reverb and broadband noise reduction.
   - Voice Isolation separates a voice. Broadband noise reduction is the room and the hiss.
   - Keyframes of effect values.
   - Hover Scrub Preview of transitions, effects and generators (pp. 759, 765, 766).
   - Onscreen controls of effects in the viewer (p. 766).
   - Favorites (p. 760).
