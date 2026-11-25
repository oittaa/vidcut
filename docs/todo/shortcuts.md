# Keyboard shortcuts

**When:** not decided.

The user can change the keys ([spec §11](../spec.md#11-keyboard-shortcuts)). These parts of the manual are still to do.

**Key files:**

- Case 13 of shortcuts-resolve-runs.md measured the key file of Resolve Studio 21.2.
- Resolve writes a key file as lines of `command := Key | Key`, with the command names of Resolve.
  - The file starts with `import "DaVinci Resolve"` and holds only the changed commands.
  - vidcut writes a TOML file with the same content, with the names of its actions.
  - To do: read and write the format of Resolve, after the built-in sets.

**Meta in the sets of other editors:**

- The Apple Final Cut Pro X preset gives Meta+M to Delete Marker. On macOS, Meta is Control. On the other systems, vidcut has no such modifier.
- A set file has one form for all systems, so the set gives Delete Marker no key.
- To do: give Delete Marker Control+M on macOS.

**Key sets and panels:**

- The Keyframes tab and the keyframe tray take Copy, Cut, Paste and Delete while they have the focus, through the application actions. A level of their own would let the user give them other keys (pp. 123–124).

**Keys that move the clip selection:**

- Ctrl+Left, Ctrl+Right, Ctrl+Up and Ctrl+Down move the clip selection in the manual (pp. 825, 900).
- The manual does not tell which clip counts as previous or above.
- To do: measure that rule, then build the four keys.
