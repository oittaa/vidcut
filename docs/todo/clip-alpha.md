# A clip's own alpha

**When:** not decided.

The spec:

- An export writes alpha only with Export Alpha on, in a format that has alpha (p. 1205).
- When a program renders as one file, its layers composite into one picture (p. 473).

In vidcut:

- A clip with alpha composites over the tracks below it by its Alpha Mode. The viewer and the exports draw it with one shader.
- An export writes opaque frames. ProRes 4444 has an opaque alpha plane on the CPU, and no alpha plane on VideoToolbox.
- Retime Process with Frame Blend or AI and Super Scale with AI drop the alpha.
- An HDR conversion into an RGB layout drops the alpha, for example in `frame.render` of a media.

To do:

- **Export Alpha** in the export settings, for formats with alpha.
  - ProRes 4444 on VideoToolbox needs AYUV64 for alpha.
  - With it off, the export composites over black. It writes opaque frames.
- **Alpha through Retime Process, Super Scale and the HDR conversion into RGB.**
