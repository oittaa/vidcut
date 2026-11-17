# A Color page

**When:** not decided.

A page for the grade of a clip, with its own nodes, as the Color page of Resolve. The Effects page holds the effects and the masks of a clip ([spec §8.19](../spec.md#819-compositions-pp-1332-1385-1403-1420-21082111-2472-2480-2483-24902492-2500)).

**Why:**

- A grade with more than one correction needs an order. On the Color page, the order of the nodes sets the order of the corrections (p. 3385).
- The effects and the grade are then apart, as in Resolve.

**What the manual says:**

- Each clip has one node in the Node Editor of the Color page. The user can add more nodes, and each node holds one or more corrections (p. 3385).
- The Node Editor of the Color page is not the node editor of the Fusion page. It has corrector nodes, serial nodes, the Parallel Mixer, the Layer Mixer and the Key Mixer (pp. 3409–3417).
- The clip grade comes after the Fusion effects and the effects of the Edit page ([spec §13](../spec.md#13-order-of-operations-pp-287296-3407)). Thus the grade of a clip comes after its composition on the Effects page.
- The Primaries palette holds the controls of a primary correction ([spec §14](../spec.md#14-primary-grade-pp-31413157)).

**In the code:**

- vidcut has no grade ([primary-grade.md](primary-grade.md)).
- The node editor of the Effects page is in `crates/vidcut/src/gui/effects_page.rs`.

**To do:**

- Measure the serial node, the Parallel Mixer and the Layer Mixer. The script interface of Resolve cannot add a node, so a grade made in the window is necessary.
- A Node Editor for the Color page, with the corrector node of [primary-grade.md](primary-grade.md).
- The CLI, IPC and MCP perform the same action as the GUI.
