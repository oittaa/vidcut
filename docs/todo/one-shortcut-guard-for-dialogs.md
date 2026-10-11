# One shortcut guard for all dialogs

**When:** later (small).

Timeline shortcuts are read in `Workspace::keys()` before the windows are drawn, so a modal dialog doesn't stop them. The numbered-images question drops them with its own guard (`gui/import.rs`); the OTIO import questions and the quit-while-exporting question still let them through. One check in `keys()` (skip shortcuts while any of the workspace's modals is open) would cover all of them, and the import question's guard could go.
