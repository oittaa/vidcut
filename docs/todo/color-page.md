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
- It also draws the Node Editor of the grade, at the right of the viewer. `crates/vidcut/src/gui/node_editor.rs` is the editor of the Color page and of the Effects page.
- On the Color page, the keys of the nodes (`Panel::Color`) go before the keys of the other pages. Thus Shift+S, Ctrl+D, Alt+D and Delete act on the nodes there.
- A Clip and Timeline switch at the top right selects the grade that the palette shows (pp. 3404–3405). It sends `gui.page` with `grade`.
- In the bars, each Offset bar has a Plus and a Minus button. Each button moves its channel by one printer point (pp. 3157, 3159).
- The Color menu has the Printer Light Hotkeys and the Full, Half and Quarter printer points. The keys of the numeric keypad move printer points (p. 3158).
- The page puts the palette at the bottom and the timeline above it, at its smallest height. The viewer gets the rest.
- A grade is a tree of correctors and mixers (`model::Grade`, [spec §8.21](../spec.md#821-color-page-nodes-pp-33853399-34093415)).

**To do:**

- The media panel on this page: its border runs past the panel when the top of the window is short.
- A click on the number of a node turns the node off or on (p. 3392).
- The current node of each clip stays when the clip changes, with the setting "Switching clips" (p. 3391).
- Previous Node and Next Node (Option-Shift-Semicolon, Option-Shift-Apostrophe, p. 3391).
- Change Label, the thumbnails and the badges (p. 3390).
- Add Outside Node (Option-O), and Node + CPW, LPW, PPW and PCW (p. 3394). They need windows.
- Insert a node with no connection by a drop on a connection (pp. 3396–3397).
- Swap two nodes with a Command-drag, copy the settings with an Option-drag, and Copy and Paste of node settings (p. 3397).
- Cleanup Node Graph, Toggle Display Mode, Zoom to Window and the Layer List view (pp. 3386–3387, 3398).
- The Nodes submenu of the Color menu (pp. 3392, 3394). The order of its items needs a measurement.
- The key input and output of each node, the Key Mixer, and Key Output Gain and Offset. Measured in Resolve Studio 21.2 with float renders:
  - smax(a, b) = max(a, b) + 0.5 max(0, 0.5 − |a − b|)². L(x) = (8x − 1) / 7. U(a, b) = L(smax(a, b)). Q(x) = U(0, x).
  - Key Mixer input i with key kᵢ: xᵢ = gᵢ kᵢ + oᵢ (Gain and Offset of the input). Invert gives 1 − xᵢ.
  - The Mask inputs go first, in input order: acc = U(acc, xᵢ), from acc = 0. Then each Matte input: acc = 1 − U(1 − acc, 1 − xᵢ).
  - The output is og acc + oo. Output Invert gives 1 − (og acc + oo). There is no clamp between the steps.
  - A key on a link is clamped to 0..1 first.
  - A corrector key input with no window gives clamp(Q(x)), with x = ig k + io. With a window w in the node, a = Q(w): Matte gives clamp(1 − U(1 − a, 1 − x)), Mask gives clamp(U(a, x)).
  - Key Output Gain kg and Key Output Offset ko of a node act last on its key E: clamp(kg E + ko).
  - In a Layer Mixer, Key Output Gain k of input 0 gives x + k (n₀ − x). Each next input gives mix + k (F(mix, nᵢ) − mix).
  - In a Parallel Mixer, out = x + Σ kᵢ (nᵢ − x).
  - The errors are below 8.2e-7. The record is in `fixtures/color/NOTES.md` of the test data.
- Measure in Resolve, then match:
  - A Parallel Mixer whose inputs do not all start from the same picture.
  - Delete Node on a mixer: what then takes the picture of the nodes after it.
  - A mixer that is turned off.
  - Lighter Color and Darker Color in a Layer Mixer when the two sums are equal.
  - Alt+P and Alt+L at a corrector on an input of a mixer that is not the last input.
  - A click on the empty Node Editor: does the current node stay.
  - An input of a mixer with no connection, from Add One Input: does the grade work.
  - Do the mixers show a number in the Node Editor.
  - Ctrl+D with selected nodes of which some are on and some are off. vidcut does nothing.
  - Delete with more than one selected node. vidcut does nothing.
  - Alt+D when each corrector is off by Ctrl+D. vidcut turns them all on.
