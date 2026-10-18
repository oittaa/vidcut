# Check Import Media Folder against Resolve

**When:** next, while the Resolve test machine is free.

Import a folder on Resolve Studio's Cut page and compare with [spec D53](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions): use a folder with a subfolder two levels down, an empty folder, a folder of documents only, numbered PNGs, a text file and a broken `.mp4`.
- The button, its tooltip and the menu items (File, the pool's right-click menu), and any shortcut.
- Which folders become bins: empty ones, ones holding no media.
- What happens to files that aren't media: skipped silently, or a message.
- The clips' order, and which video's frame rate the empty project is offered.
- Importing the same folder again: a second bin, or the same one.

Record what differs in [deviations.md](../deviations.md) or change D53 and the code.
