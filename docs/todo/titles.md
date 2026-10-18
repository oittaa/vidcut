# Titles

**When:** not decided; after subtitles, if those are taken up.

The Cut page adds a title from its Titles browser as a clip: 5 s long by default, moved and trimmed like any other, with the Transform, Cropping, Speed and Composite controls every clip has. Its text is edited in the viewer or the Video Inspector (pp. 763–764). The spec leaves titles out ([§1](../cut-page-spec.md#1-scope)), and an OTIO file's titles are carried and written back unchanged ([deviation X9](../deviations.md#x9-opentimelineio-interchange)).

What titles would take:
- **A title clip:** text, font, size, color and position, on a background or none, on any video track, edited, trimmed and moved like a clip, and scriptable (AGENTS.md rule 3).
- **The same text in the viewer and the export**, drawn on the GPU (rule 9): rendered once per title and picture size, and laid over the clips as a dissolve's layers are.
- **OTIO both ways:** Resolve's Text and Text+ titles read as vidcut titles where they fit; the rest carried as now.
- **Which titles first:** plain text and a lower third, checked against what the Cut page's Titles browser offers.
