# The Color page

**When:** not decided.

The Color page shows the grade of the clip at the playhead or of the timeline. It has the Node Editor, the palette of the current node, the viewer and the timeline.

**Why:**

- The key channel of a node limits its corrections to a part of the picture (p. 3388).
- The Key Mixer and Key Output Gain need those keys (pp. 3389, 3415).

**What the manual says:**

- Each node has a key input and a key output. They are blue, at the bottom left and right of the node (p. 3388).
- Nodes can have a label. Each node shows a thumbnail and badges of its palettes (p. 3390).
- The palettes are at the bottom of the page, below the timeline (pp. 3046, 3053, 3055).

**In the code:**

- `crates/vidcut/src/gui/color_page.rs` draws the Primaries wheels, the Primaries bars and the Log wheels of the current node. It sends `grade.set`, `grade.clear` and `grade.printer` with `node`.
- Its Key palette has the Key Input and the Key Output of the current node (`KeyInput`, `KeyOutput`). A key mixer has its own Key palette.
- Its Qualifier palette has the HSL qualifier of the current node (`Qualifier`). The Key palette has the Qualifier row of the node.
- The key ports, the key links, the Key Mixer, the Outside node, the Splitter and the Combiner are in the Node Editor.
- Its Window palette has the windows of the current node (`grade_windows.rs`). It sends the `grade.window` requests.
- With the Window palette open, the viewer shows the outlines and the handles of the windows (`window_overlay.rs`).
- It also draws the Node Editor of the grade, at the right of the viewer. `crates/vidcut/src/gui/node_editor.rs` is the editor of the Color page and of the Effects page.
- Each corrector in the Node Editor and in List View shows its thumbnail (`Shot`, [live.md](../design/live.md)).
- On the Color page, the keys of the nodes (`Panel::Color`) go before the keys of the other pages. Thus Shift+S, Alt+B, Ctrl+D, Alt+D and Delete act on the nodes there.
- A Clip and Timeline switch at the top right selects the grade that the palette shows (pp. 3404–3405). It sends `gui.page` with `grade`.
- In the bars, each Offset bar has a Plus and a Minus button. Each button moves its channel by one printer point (pp. 3157, 3159).
- The page puts the palette at the bottom and the timeline above it, at its smallest height. The viewer gets the rest.
- A grade is a tree of correctors and mixers (`model::Grade`, [spec §8.21](../spec.md#821-color-page-nodes-pp-33853399-34093415)).
- The palette and the timeline start at 60 percent of the window height at most (`BOTTOM_SHARE`). They also leave the media panel the height of its rows and a short list (`pool_least`).
  - The palette scrolls when it needs more height.
  - A drag cannot make the timeline taller than that limit.

**To do:**

- Measure the node numbers after an Extract, and the order of the moved links after a delete or an Extract, then match them ([spec §8.21](../spec.md#821-color-page-nodes-pp-33853399-34093415)).
  - vidcut gives each moved link the order of the link into the removed node. No measurement shows that this is correct.
- Measure where Alt+K draws its new node, then match it. In one case, it went to a free place under node 01 ([spec §8.21](../spec.md#821-color-page-nodes-pp-33853399-34093415)).
- The 3D, RGB and Luma modes of the Qualifier palette (pp. 3241–3242, 3254–3258).
- The pickers of the Qualifier palette: the eyedropper, Add and Subtract Color Range, and Add and Subtract Softness (p. 3247).
- Matte Finesse of the Qualifier palette (pp. 3259–3262). It needs a Resolve run first.
  - The record has its usual values, its ranges and eleven renders on HSL keys (Q1 M1 to M11). Their math is not fitted.
  - Fit Blur Radius, In/Out Ratio, Black Clip, White Clip, Clean Black, Clean White and the Morph operations from them first.
- Measure Reset in the Qualifier palette and in the Key palette, and the keyframe track of a qualifier change, then match them ([spec §8.28](../spec.md#828-qualifier-pp-32383256-3263-3436-measured-in-resolve-studio-212)).
- Measure Add Serial in the menu of a Splitter, which has three outputs, then match it ([spec §8.21](../spec.md#821-color-page-nodes-pp-33853399-34093415)).
- Input Sizing, a mode of the Sizing palette (pp. 3497–3500). It acts before the node graph (p. 3498).
  - The Sizing mode and the Sizing track of the Keyframe Editor need it (pp. 3455, 3460).
  - Measure its order against the Transform of the Edit page first.
