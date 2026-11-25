# Keyboard shortcuts

**When:** not decided.

The user can change the keys ([spec §11](../spec.md#11-keyboard-shortcuts)). These parts of the manual are still to do.

**Key files to measure:**

- Case 27 of shortcuts-resolve-runs.md: how Resolve reads a key file that vidcut writes.
- Does Resolve read Esc and Ins, the comment lines of vidcut, and a key that another command has?
- Which import line does Resolve write for a preset that it saved from another built-in preset?

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
