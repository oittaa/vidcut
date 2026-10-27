# A clip's own alpha

**When:** not decided.

The spec:

- A clip with an alpha channel composites over the tracks below it, with no setting (p. 769, p. 1205).
- Alpha channels are on by default (p. 473).
- Clip Attributes has Alpha Mode: None, Premultiplied or Straight, and Invert (p. 451).
- An export writes alpha only with Export Alpha on, in a format that has alpha (p. 1205).
- When a program renders as one file, its layers composite into one picture (p. 473).

Now, in vidcut:

- The viewer and the exports use only a clip's luma and chroma. A clip with alpha covers the tracks below it fully.
  - Measured: red at an alpha of 0.25 over green exports as red (255, 0, 1), in H.264 and in ProRes 4444.
- A ProRes 4444 export on the CPU (`prores_ks`) writes the alpha of the top clip as it is.
  - Measured: a clip at an alpha of 0.27 exports at 0.27, also over a green clip on the track below.
- A ProRes 4444 export on VideoToolbox writes no alpha plane (P416).
- VideoToolbox decodes ProRes 4444 with alpha into AYUV64. The viewer shows it from GPU memory, and its alpha is in the frame.

To do:

- **Composite by alpha** in the viewer, in the exports on the CPU, and in the mixes of CUDA and Metal.
  - Measure in Resolve how it treats a ProRes 4444 clip whose tags say nothing of premultiplied or straight alpha.
- **Alpha Mode** in the clip's attributes, with None, Premultiplied, Straight and Invert.
- **Export Alpha** in the export settings, for formats with alpha: ProRes 4444, and on VideoToolbox in AYUV64.
  - With it off, the export composites over black and writes opaque frames.
