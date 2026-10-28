# A clip's own alpha

**When:** not decided.

The spec:

- A clip with an alpha channel composites over the tracks below it, with no setting (p. 769, p. 1205).
- Alpha channels are on by default (p. 473).
- Clip Attributes has Alpha Mode: None, Premultiplied or Straight, and Invert (p. 451).
- An export writes alpha only with Export Alpha on, in a format that has alpha (p. 1205).
- When a program renders as one file, its layers composite into one picture (p. 473).
- Measured in Resolve Studio 21.1.1: a ProRes 4444 with alpha and no tags is Straight.
- Measured in Resolve Studio 21.1.2, with 16-bit renders. A PNG of color c and alpha α is over a picture b.
  - None: c. The PNG is opaque.
  - Straight: b + (c − b) × α.
  - Premultiplied: c + b × (1 − α), at most 1.
  - The opacity and the Softness of the crop multiply α.
  - A composite mode mixes first. Then α fades the mix M: b + (M − b) × α.
  - Invert: b + (c − b) × (1 − α). It is the fourth choice of the Alpha Mode list.

In vidcut:

- A clip with alpha composites over the tracks below it by straight alpha. The viewer and the exports draw it with one shader.
- An export writes opaque frames. ProRes 4444 has an opaque alpha plane on the CPU, and no alpha plane on VideoToolbox.
- Retime Process with Frame Blend or AI and Super Scale with AI drop the alpha.
- An HDR conversion into an RGB layout drops the alpha, for example in `frame.render` of a media.

To do:

- **Alpha Mode** in the clip's attributes, with None, Premultiplied, Straight and Invert.
- **Export Alpha** in the export settings, for formats with alpha.
  - ProRes 4444 on VideoToolbox needs AYUV64 for alpha.
  - With it off, the export composites over black. It writes opaque frames.
- **Alpha through Retime Process, Super Scale and the HDR conversion into RGB.**
- **Check by hand:** red at an alpha of 0.25 over green, in an export.
