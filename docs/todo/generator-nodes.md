# Generator nodes in compositions

**When:** in progress. The Background and the Fast Noise tools are in.

The compositions get the generator nodes of the Fusion page (pp. 2356–2405).

**Why:**

- A composition that makes its own pictures needs the generators of the Fusion page.
- The Background and the Fast Noise tools are on the default toolbar (p. 1304).

**What Resolve has, and vidcut does not have yet:**

- The Plasma (pp. 2379–2380), Day Sky (pp. 2360–2362) and Mandelbrot (pp. 2366–2367) tools.
  - The reads of Resolve Studio 21.2 are in sections M14 and M15 of `fixtures/nodes/NOTES.md` in the test data.
  - Mandelbrot: Grad Method is Continuous Potential or Iterations. Usual: Continuous Potential.
  - Mandelbrot: Gradient Curve, usual 0.125. With Continuous Potential, one point reads 0.69726 at 0.125, 0.70278 at 0.25, 0.71373 at 0.5 and 0.73504 at 1.0.
  - Mandelbrot: the scale of the pattern follows the width of the picture.
  - Plasma: Operation is Add, Subtract, Multiply, Divide, Maximum, Minimum or Difference. Usual: Add.
  - Plasma: the size of the rings follows the width of the picture.
- The Background tool:
  - Interpolation Space of the gradient (p. 2359). The spec gives the measured hue turn, HLS, HSV and LAB.
  - Keyframes on the stops of the gradient, and its contextual menu (p. 2360). The spec gives the measured menu and the blend of the stops.
- The Fast Noise tool:
  - The noise function of Resolve. vidcut uses fractal Perlin noise on the measured grid.
  - Gradient Type of its gradient: seven items, and the usual item is Uni ([spec](../spec.md#the-fast-noise-tool)).
  - Interpolation Space of its gradient, as on the Background.
- A picture on a map input of the Fast Noise. vidcut takes masks there.
- The Image tab of the generators (pp. 2401–2402):
  - Width and Height without Auto Resolution. The fields are kept, but the picture has the size of the clip's own picture.
  - Process Mode, Global In and Out, Pixel Aspect, Depth, Source Color Space and Source Gamma Space.

**To measure in Resolve:**

- Seethe Rate of the Fast Noise. The level at frame 5 is not the level of any Seethe value that was read ([spec](../spec.md#the-fast-noise-tool)).
- What the Gradient Type items of the Fast Noise other than Uni do.
- The map inputs of the Fast Noise. A link to them did not join in two measurements.
