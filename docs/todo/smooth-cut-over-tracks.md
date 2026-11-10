# A Smooth Cut over other tracks

vidcut plays a Smooth Cut as a Cross Dissolve when a side is not opaque over the whole frame and other tracks are below it.

Resolve Studio 21.2.0 morphs this case:

- The morph takes each side as a picture with alpha. A bar is black in the morph.
- The alpha of the morph is the alpha of the two sides, mixed by the level of the transition.
- The tracks below show through by that alpha.

To do:

- Give the made frame an alpha. `interpolate::between` makes RGB only.
- Then remove the `covers` test in `render::Span::flat`.
