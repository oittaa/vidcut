# Generator nodes in compositions

**When:** in progress. The Background and the Fast Noise tools are in.

The compositions get the generator nodes of the Fusion page (pp. 2356–2405).

**Why:**

- A composition that makes its own pictures needs the generators of the Fusion page.
- The Background and the Fast Noise tools are on the default toolbar (p. 1304).

**What Resolve has, and vidcut does not have yet:**

- The Plasma (pp. 2379–2380), Day Sky (pp. 2360–2363) and Mandelbrot (pp. 2366–2367) tools.
- The Background tool:
  - Interpolation Space of the gradient (p. 2359).
  - Sub-Pixel of the gradient (p. 2360).
  - Keyframes on the stops of the gradient, and its contextual menu (p. 2360).
- The Fast Noise tool:
  - The noise of Resolve. vidcut uses fractal Perlin noise. It agrees with the measurement at Detail 0 only.
  - Gradient Type of its gradient. The usual item is Uni. The other items are not measured yet.
  - Interpolation Space, Offset, Repeat and Sub-Pixel of its gradient, as on the Background.
- A picture on a map input of the Fast Noise. vidcut takes masks there.
- The Image tab of the generators (pp. 2401–2402):
  - Width and Height without Auto Resolution.
  - Process Mode, Global In and Out, Pixel Aspect, Depth, Source Color Space and Source Gamma Space.

**To measure in Resolve:**

- The math of the fills and the gradients, and Width and Height without Auto Resolution.
- The noise of the Fast Noise with Detail above 0, and its Gradient Type items.
- The plan: steps G2 b to l and the renders of G4, in M13 of the measurement notes of the nodes (`fixtures/nodes`).
