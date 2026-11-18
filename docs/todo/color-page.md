# The Color page

**When:** not decided.

The Color page shows the primary correction of the clip at the playhead, the viewer and the timeline. Resolve's Color page also has nodes.

**Why:**

- A grade with more than one correction needs an order. On the Color page, the order of the nodes sets the order of the corrections (p. 3385).
- The effects and the grade are then apart, as in Resolve.

**What the manual says:**

- Each clip has one node on the Color page. The user can add more nodes, and each node holds one or more corrections (p. 3385).
- The Primaries palette shows the same controls as wheels or as bars (pp. 3142, 3151).
- The palettes are at the bottom of the page, below the timeline (pp. 3046, 3053, 3055).

**In the code:**

- `crates/vidcut/src/gui/color_page.rs` draws the Primaries wheels, the Primaries bars and the Log wheels. It sends `grade.set` and `grade.clear`.
- In the bars, each Offset bar has a Plus and a Minus button. Each button moves its channel by one printer point (pp. 3157, 3159).
- The page puts the palette at the bottom and the timeline above it, at its smallest height. The viewer gets the rest.
- The palette and the timeline take at most 60 percent of the window height, as the timeline of the Cut page does (`BOTTOM_SHARE`). The palette scrolls when it needs more.
- Each clip has one correction (`model::Grade`, [primary-grade.md](primary-grade.md)).
- The node editor of the Effects page is in `crates/vidcut/src/gui/effects_page.rs`.

**To do:**

- The Printer Light Hotkeys of the Color menu, and the Full, Half and Quarter printer points with their keys (p. 3158).
- Nodes: more than one correction on a clip, in the order of the nodes.
- Decide whether the Color page uses the node editor of the Effects page.
