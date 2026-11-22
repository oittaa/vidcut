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
- The key ports, the key links, the Key Mixer and the Outside node are in the Node Editor.
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

- The windows ([spec §8.23](../spec.md#823-windows-pp-32653279)):
  - Shift and a drag box select points of a curve, to move, delete or scale them together (p. 3274, measured).
- Switch to Node Layer in the Nodes submenu, with the Node Stack Layers (pp. 3400–3401). Add Splitter Combiner Node (Option-Y, p. 3441).
- The Qualifier controls of the Key palette (p. 3436).
- Measure in Resolve, then match:
  - The numbers of new nodes ([spec §8.21](../spec.md#821-color-page-nodes-pp-33853399-34093415)). One rule fits 5 cases. Measure Alt+P, Alt+L, Alt+K and deletes before vidcut uses it.
