# Generator nodes in compositions

**When:** in progress. The Background tool is in the first pull request.

The compositions get the generator nodes of the Fusion page (pp. 2356–2405).

**Why:**

- A composition that makes its own pictures needs the generators of the Fusion page.
- The Background and the Fast Noise tools are on the default toolbar (p. 1304).

**What Resolve has, and vidcut does not have yet:**

- The Fast Noise tool (pp. 2363–2366), with its Noise Detail Map and Noise Brightness Map inputs (p. 2403).
- The Plasma (pp. 2379–2380), Day Sky (pp. 2360–2362) and Mandelbrot (pp. 2366–2367) tools.
- The Background tool:
  - Interpolation Space of the gradient (p. 2359). The spec gives the measured hue turn and LAB.
  - Sub-Pixel of the gradient (p. 2360). The spec gives the measured samples.
  - Keyframes on the stops of the gradient, and its contextual menu (p. 2360).
- The Image tab of the generators (pp. 2401–2402):
  - Width and Height without Auto Resolution. The fields are kept, but the picture has the size of the clip's own picture.
  - Process Mode, Global In and Out, Pixel Aspect, Depth, Source Color Space and Source Gamma Space.

**To measure in Resolve:**

- Before Interpolation Space is built: HLS and HSV with a gray stop, and with two colors of a different lightness. The reads cover red to blue only.
- Angle, Square and Cross with End not right of Start. The reads have End right of Start only.
