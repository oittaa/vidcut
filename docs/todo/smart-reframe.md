# Smart Reframe

**When:** not decided.

Smart Reframe is later Cut page work ([spec §1](../spec.md#1-scope)). vidcut does not have it.

**Why:**

- The manual's list of Cut page tools marks Smart Reframe as Studio.
- The `tiktok` preset writes 1080×1920 (`builtin_presets`).
- The viewer draws a 9:16 guide when that guide is on (`Framing::Social9_16`).
- The user still sets the crop.

**In the code:**

- `Picture` holds Transform, Crop and Dynamic Zoom.
- Dynamic Zoom moves from a start box to an end box. The user sets both.
- On-screen handles move, zoom and turn a clip (`onscreen`).

**To do:**

- Smart Reframe keeps a person, or a point the user chooses, inside the framing aspect.
- Store that move as Transform keyframes when the measured values fit them.
- The analysis runs on this computer.
- A missing model uses the download check and the SHA-256 check of the other local models.
- The CLI, IPC and MCP perform the same action as the GUI.
