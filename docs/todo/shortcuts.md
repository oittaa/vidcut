# Keyboard shortcuts

**When:** not decided.

The user can change the keys ([spec §11](../spec.md#11-keyboard-shortcuts)). These parts of the manual are still to do.

**Key sets, open differences:**

- The owner decides each item. Cases 13 and 15 of shortcuts-resolve-runs.md measured them in Resolve Studio 21.2.
- In Resolve, the changes and the set picks in the window are a draft until Save.
  - With a draft, the window has Cancel and Save. Cancel and a close ask "Discard Changes?".
  - When the window opens again, it shows the keys that were saved last.
  - vidcut puts each change and each set pick in force at once. The preferences keep the changes.
- Resolve writes a key file as lines of `command := Key | Key`, with the command names of Resolve.
  - The file starts with `import "DaVinci Resolve"` and holds only the changed commands.
  - vidcut writes a TOML file with the same content, with the names of its actions.

**Meta in the sets of other editors:**

- The Apple Final Cut Pro X preset gives Meta+M to Delete Marker. On macOS, Meta is Control. On the other systems, vidcut has no such modifier.
- A set file has one form for all systems, so the set gives Delete Marker no key.
- To do: give Delete Marker Control+M on macOS.

**Key sets and panels:**

- Keys per panel. One key does different things in different panels (pp. 123–124).
  - Only the Transcription window has its own level.

**Keys that move the clip selection:**

- Ctrl+Left, Ctrl+Right, Ctrl+Up and Ctrl+Down move the clip selection in the manual (pp. 825, 900).
- The manual does not tell which clip counts as previous or above.
- To do: measure that rule, then build the four keys.

**Effects Search:**

- Shift+Space is Effects Search on the Cut page (p. 1144).
- vidcut has no Effects Search.
