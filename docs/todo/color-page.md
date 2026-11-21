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
- The page puts the palette at the bottom and the timeline above it, at its smallest height. The viewer gets the rest.
- A grade is a tree of correctors and mixers (`model::Grade`, [spec §8.21](../spec.md#821-color-page-nodes-pp-33853399-34093415)).
- The palette and the timeline start at 60 percent of the window height at most (`BOTTOM_SHARE`). They also leave the media panel the height of its rows and a short list (`pool_least`).
  - The palette scrolls when it needs more height.
  - A drag cannot make the timeline taller than that limit.

**To do:**

- The thumbnails (p. 3390), also in the rows of List View.
- Add Outside Node (Option-O), and Node + CPW, LPW, PPW and PCW (p. 3394). They need windows.
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
  - A Layer Mixer that is off keeps the Key Output Gain of its inputs.
- Lock Node and Remove One Input in the menu of a mixer. Measure first what each one does. List View then shows the lock.
- Measure in Resolve, then match:
  - Remove One Input: which input goes, and the least number of inputs.
  - Lock Node: what a locked node refuses.
  - The items of the Add Node submenu of a mixer, and the menu of a Parallel Mixer.
  - Delete after a drag box that does not hold the current node: does the current node go too.
  - A Ctrl-click on a node that is selected already.
