# egui: the keys of the numeric keypad

**When:** once egui-winit passes on whether a key comes from the numeric keypad.

**Why:**

- egui-winit maps `KeyCode::Numpad7` and `KeyCode::Digit7` to the same `egui::Key`. It ignores `KeyLocation`. egui's main branch does the same.
- The printer point keys need the keys of the numeric keypad alone (p. 3158, [spec §8.20](../spec.md#820-primaries-palette-pp-118-31413159)). The keys of the main keyboard keep their own actions.

- The built-in set of Resolve Studio 21.2 also gives keys of the numeric keypad to three actions:
  - Num+= to Go to a Timecode.
  - Num++ to Jump Forward.
  - Num+- to Jump Back.

**To do:**

- The keys of the numeric keypad for the Full and Quarter printer points, and the Printer Light Hotkeys item that turns them on.
- Num+=, Num++ and Num+- in the table of vidcut's own keys, and the keys of the numeric keypad in the built-in sets of other editors.
