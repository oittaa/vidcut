# Smart Reframe and Close Up

**When:** not decided.

Smart Reframe and Close Up are later Cut page work ([spec §1](../cut-page-spec.md#1-scope), [§7.2](../cut-page-spec.md#72-edit-functions)). vidcut has neither.

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

- Measure Smart Reframe and Close Up in Resolve, including the values they store.
- Smart Reframe keeps a person, or a point the user chooses, inside the framing aspect.
- Store that move as Transform keyframes when the measured values fit them.
- Close Up fills the frame with the face, as that edit does in the manual.
- The analysis runs on this computer.
- A missing model uses the download check and the SHA-256 check of the other local models.
- The CLI, IPC and MCP perform the same action as the GUI.
