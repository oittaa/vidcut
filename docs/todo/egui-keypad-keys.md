# egui: the keys of the numeric keypad

**When:** once egui-winit passes on whether a key comes from the numeric keypad.

**Why:**

- egui-winit maps `KeyCode::Numpad7` and `KeyCode::Digit7` to the same `egui::Key`. It ignores `KeyLocation`. egui's main branch does the same.
- The printer point keys need the keys of the numeric keypad alone (p. 3158, [spec §8.20](../spec.md#820-primaries-palette-pp-118-31413159)). The keys of the main keyboard keep their own actions.

**To do:** the keys of the numeric keypad for the Full and Quarter printer points, and the Printer Light Hotkeys item that turns them on.
