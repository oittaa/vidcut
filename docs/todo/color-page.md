# A Color page

**When:** not decided.

A page for the grade of a clip, with its own nodes. The Effects page holds the effects and the masks of a clip ([spec §8.19](../spec.md#819-compositions-pp-1332-1385-1403-1420-21082111-2472-2480-2483-24902492-2500)).

**Why:**

- A grade with more than one correction needs an order. On the Color page, the order of the nodes sets the order of the corrections (p. 3385).
- The effects and the grade are then apart, as in Resolve.

**What the manual says:**

- Each clip has one node on the Color page. The user can add more nodes, and each node holds one or more corrections (p. 3385).
- The Primaries palette holds the Lift, Gamma and Gain controls of a primary correction (p. 3141).

**In the code:**

- vidcut has no grade ([primary-grade.md](primary-grade.md)).
- The node editor of the Effects page is in `crates/vidcut/src/gui/effects_page.rs`.

**To do:**

- Decide the order of the grade and the composition of a clip ([spec §13](../spec.md#13-order-of-operations-pp-287296-3407)).
- Decide whether the Color page uses the node editor of the Effects page.
- Build the corrections of [primary-grade.md](primary-grade.md) as nodes of this page.
- The CLI, IPC and MCP perform the same action as the GUI.
