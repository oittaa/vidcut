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
  - vidcut shows Blend in the one list of controls.
- **The Image tab of a mask** (p. 2500):
  - Output Size: Default or Custom.
  - Clipping Mode: None or Frame. Measured: None is the usual value.
  - The Soft Edge of vidcut repeats the edge pixels of the mask. Measure what None gives at the edge of the picture.
- **Paint Mode of a mask:** it has 11 modes, and Merge is the usual mode (p. 2474). vidcut always uses Merge.
- **Border Style of a rectangle:** measured: three choices, above Center. vidcut has none.
- **A drop on a node:**
  - When the main input is joined, Resolve joins the next free input. That can be the effect mask input (p. 1395).
  - An effect mask input also takes the picture of a tool (pp. 2463, 2472).
  - vidcut joins only an empty input, and only a mask to an effect mask input.
- **The active node after a box select:** measured: Resolve then has no active node, and the Inspector shows the headers closed. vidcut makes the last node active.
- **The Center of a Vignette:** measured: one row with an X and a Y field, and no point in the viewer. vidcut shows Center X and Center Y in two rows.
- **Page keys that vidcut does not have:**
  - Shift+Space opens Select Tool (p. 1386).
  - = and − zoom the viewer (p. 1290). Ctrl+1 sets the scale of the Node Editor (p. 1310).
  - Ctrl+F is Find (p. 1421). 1, 2 and ` put a node in a viewer (p. 1393).
  - Ctrl+L locks a node (p. 1424). Ctrl+Shift+V pastes an instance (p. 1413). Ctrl+G makes a group (p. 1427).
- The CLI, IPC and MCP perform the same action as the GUI, in each step.
