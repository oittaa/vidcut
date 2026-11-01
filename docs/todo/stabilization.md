# Stabilization

**When:** not decided.

The Cut page viewer Tools stabilize a clip. vidcut has no stabilization.

**Why:**

- The control analyzes the clip and smooths the camera motion.
- The same data is in the Video Inspector.
- The picture order puts Image Stabilization between Edit Sizing and Input Sizing ([spec §13](../cut-page-spec.md#13-order-of-operations-pp-287296-3407)).

**In the code:**

- The design lists Image Stabilization among the stages vidcut does not have.
- Transform can move a clip. It does not analyze motion.

**To do:**

- Measure the methods, the Stabilize action, and what an OpenTimelineIO file keeps.
- One request analyzes the selected clip and stores the result.
- A change of the method analyzes the clip again.
- The viewer and the export play that result.
- The CLI, IPC and MCP perform the same action as the GUI.
