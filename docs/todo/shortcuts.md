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
- Case 17 measured the keys of the built-in presets. The files are in `$VIDCUT_TEST_DATA/fixtures/keys`.

**Sets of other editors:**

- The Avid Media Composer set has 15 actions. The documents that were read show most keys as pictures.
- To do: read the Media Composer Editing Guide. It gives the keys for the marks, markers, Add Edit, Lift, Extract and trim.
- The Pro Tools set has 6 actions. Most Pro Tools keys act on a time range or depend on the edit mode, so they do not match a vidcut action.

**Key sets and panels:**

- The Keyframes tab and the keyframe tray take Copy, Cut, Paste and Delete while they have the focus, through the application actions. A level of their own would let the user give them other keys (pp. 123–124).

**Keys that move the clip selection:**

- Ctrl+Left, Ctrl+Right, Ctrl+Up and Ctrl+Down move the clip selection in the manual (pp. 825, 900).
- The manual does not tell which clip counts as previous or above.
- To do: measure that rule, then build the four keys.
