# The Effects page, what is left

**When:** not decided.

The Effects page follows the Fusion page ([spec §8.19](../spec.md#819-compositions-pp-1332-1385-1403-1420-21082111-2472-2480-2483-24902492-2500)). vidcut does not have these parts of the Fusion page yet.

**Why:** each part is a control or a key that the Fusion page has. The facts with no page were measured in Resolve Studio 21.2.0 on 2026-11-13.

**To do:**

- **The node header in the Inspector:**
  - Resolve has Set Color, Versions and Pin (pp. 1313–1314).
  - vidcut has the switch, the name, the tool, Lock and Reset.
- **The Settings tab of a tool and of a mask** (pp. 2131, 2501–2502):
  - Blend, Process When Blend Is 0.0, the channels, Apply Mask Inverted, Multiply by Mask, Motion Blur, Use GPU, Comments and Scripts.
  - vidcut shows Blend and Multiply by Mask in the one list of controls.
- **Output Size of a mask** (pp. 2500–2501, [spec](../spec.md#the-masks)): Default or Custom, with the rows of Custom.
- **Border Style of a rectangle:** the ends, Position and Length ([spec](../spec.md#the-masks)). vidcut has the joins.
- **Fit Mask of a picture on an effect mask** (p. 2465, [spec](../spec.md#the-masks)): Crop, Stretch, Inside, Width, Height and Outside. It needs pictures of other sizes, as the Image tab of the generators gives ([todo](generator-nodes.md)).
  - The reads of Resolve Studio 21.2 are in section M17 R4c of `fixtures/nodes/NOTES.md` in the test data.
- **Page keys that vidcut does not have:**
  - = and − zoom the viewer, and Ctrl+1 in a viewer sets it to 100 percent (p. 1290).
  - Ctrl+F is Find (p. 1421).
  - Ctrl+Shift+V pastes an instance (p. 1413). Ctrl+G makes a group (p. 1427).
- **The render range** (pp. 1294, 1297, [spec](../spec.md#the-effects-page)):
  - The Render Start and Render End fields at the left of the transport.
  - A Ctrl-drag on the ruler, a drag of a yellow line, and a drag of a node to the ruler.
  - Auto Render Range and Set to Output Range in the menu of the ruler.
  - Auto Render Range and a change of page give back the In and the Out, as the manual says ([gotchas](../gotchas.md#resolves-fusion-page)).
  - Play, Ctrl+Left and Ctrl+Right with the ends of the range.
  - vidcut: the render range is always the In and the Out of the clip.
- **The menu of Play Forward** (p. 1296): Playback Update and the frame increment ([spec](../spec.md#the-effects-page)).
- **The Audio button** at the left of the transport (p. 1297): Mute, and the menu that picks the source of the sound ([spec](../spec.md#the-effects-page)).
- **Sound of a clip with a speed change:** measure if the page plays its sound. vidcut plays none.
- The CLI, IPC and MCP perform the same action as the GUI, in each step.

**To measure in Resolve:**

- Keyframes on Low and High of a picture on an effect mask. vidcut keeps one value for each.
- Clear in the menu of a version button. The read did not show what it does.
- A Ctrl-drag on the ruler. In the measurement, it moved the playhead only, and Ctrl possibly did not reach Resolve.
- Position and Length of a rectangle with Solid off, on a 16:9 picture. They are known only at Position 0 and Length 0.5.
  - Read the band at Position 0.25 with Length 0.25, and at Position 0.9 with Length 0.3. These tell if Position goes by the length of the edge or by the sides, and what occurs past 1.
  - Read the three ends at Length 0.5 with Border Width 0.05.
- The joins of a rectangle with Corner Radius 0.2 and Border Width 0.1. vidcut keeps the round corners for each join.
- What a version of a node keeps. Change Size on version 2, click version 1, and click version 2 again.
  - Read if the keyframes, the switch, Lock and Set Color change with the version.
- The color of the tile of each tool that vidcut has, after Clear Color. vidcut gives each tile one color.
  - The tools: Blur, Gaussian Blur and the other five Resolve FX, Merge, Text+, Background, Fast Noise, Tracker, the three masks, MediaIn and MediaOut.
- The rows of the Settings tab of each of these tools.
  - Read Apply Mask Inverted with Multiply by Mask on, and Red off in the Settings tab of a Gaussian Blur.
- Play with a frame increment of 5 and a render range of 10 to 30, from frame 12. Read the time between the frames that show.
- Right with a frame increment of 0.5 from frame 20. Read the frame that the viewer and the Current Time field show.
- Playback Update Fast and Full. Read what changes in the viewer while it plays.
- = and − in a viewer. Read the scale after one, two and three presses, from Fit and from 100 percent.
- A drag of a Blur node and of a Background node to the ruler. Read the render range that each sets.
