# A clip's own alpha

**When:** not decided.

In vidcut:

- A clip with alpha composites over the tracks below it by its Alpha Mode. The viewer and the exports draw it with one shader.
- Retime Process with Frame Blend or AI and Super Scale with AI drop the alpha.
- An HDR conversion into an RGB layout drops the alpha, for example in `frame.render` of a media.

To do:

- **Alpha through Retime Process, Super Scale and the HDR conversion into RGB.**
