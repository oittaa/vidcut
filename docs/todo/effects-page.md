# The Effects page, what is left

**When:** not decided.

The Effects page follows the Fusion page ([spec §8.19](../spec.md#819-compositions-pp-1332-1385-1403-1420-21082111-2472-2480-2483-24902492-2500)). vidcut does not have these parts of the Fusion page yet.

**Why:** each part is a control or a key that the Fusion page has. The facts with no page were measured in Resolve Studio 21.2.0 on 2026-11-13.

**To do:**

- **The node header in the Inspector:**
  - Resolve has Set Color, Versions, Pin, Lock and Reset (pp. 1313–1314), and a switch that turns the node on and off (p. 1424).
  - vidcut has the name, the tool and Reset.
- **The Settings tab of a tool and of a mask** (pp. 2131, 2501–2502):
  - Blend, Process When Blend Is 0.0, the channels, Apply Mask Inverted, Multiply by Mask, Motion Blur, Use GPU, Comments and Scripts.
  - vidcut shows Blend and Multiply by Mask in the one list of controls.
- **The Image tab of a mask** (pp. 2500–2501, [spec](../spec.md#the-masks)):
  - Output Size: Default or Custom, with the rows of Custom.
  - Clipping Mode: None or Frame. None is the usual value.
  - The Soft Edge of vidcut repeats the edge pixels of the mask, as Frame does. None makes the mask beyond the frame.
- **Paint Mode of a mask:** it has 11 modes, and Merge is the usual mode (p. 2474). The spec gives the measured forms ([spec](../spec.md#the-masks)).
  - Replace follows the manual: the areas where the new mask is 0 keep the input mask (p. 2465, [gotchas](../gotchas.md#resolves-fusion-page)).
  - vidcut always uses Merge.
- **Border Style of a rectangle:** the joins, the ends, Position and Length ([spec](../spec.md#the-masks)). vidcut has none.
- **A drop on a node:**
  - When the main input is joined, Resolve joins the next free input. That can be the effect mask input (p. 1395). The spec gives the measured drops ([spec](../spec.md#edits)).
  - An effect mask input also takes the picture of a tool (pp. 2463, 2472). Its Settings tab then has Fit Mask, Channel, a Low and High range, and Clip ([spec](../spec.md#the-masks)).
  - The Luminance and the Hue channels use the Rec. 601 luma and the true hue ([gotchas](../gotchas.md#resolves-fusion-page)).
  - vidcut joins only an empty input, and only a mask to an effect mask input.
- **The active node after a box select:** measured: Resolve then has no active node, and the Inspector shows the headers closed. vidcut makes the last node active.
- **A delete of a viewed node:** measured: Resolve puts the node on its input in that viewer ([spec](../spec.md#the-effects-page)). vidcut empties the viewer.
- **The Center of a Vignette:** measured: one row with an X and a Y field, and no point in the viewer. vidcut shows Center X and Center Y in two rows.
- **Page keys that vidcut does not have:**
  - Shift+Space opens Select Tool (p. 1386).
  - = and − zoom the viewer (p. 1290). Ctrl+1 sets the scale of the Node Editor (p. 1310).
  - Ctrl+F is Find (p. 1421).
  - Ctrl+L locks a node (p. 1424). Ctrl+Shift+V pastes an instance (p. 1413). Ctrl+G makes a group (p. 1427).
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

- Clear in the menu of a version button. The read did not show what it does.
- A Ctrl-drag on the ruler. In the measurement, it moved the playhead only, and Ctrl possibly did not reach Resolve.
