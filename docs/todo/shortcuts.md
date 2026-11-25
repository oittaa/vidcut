# Keyboard shortcuts

**When:** not decided.

The user can change the keys ([spec §11](../spec.md#11-keyboard-shortcuts)). These parts of the manual are still to do.

**Key files:**

- Case 13 of shortcuts-resolve-runs.md measured the key file of Resolve Studio 21.2.
- Resolve writes a key file as lines of `command := Key | Key`, with the command names of Resolve.
  - The file starts with `import "DaVinci Resolve"` and holds only the changed commands.
  - vidcut writes a TOML file with the same content, with the names of its actions.
  - To do: read and write the format of Resolve, after the built-in sets.
- Case 17 measured the keys of the built-in presets. The files are in `$VIDCUT_TEST_DATA/fixtures/keys`.

**Sets of other editors:**

- The Avid Media Composer set has 15 actions. The documents that were read show most keys as pictures.
- To do: read the Media Composer Editing Guide. It gives the keys for the marks, markers, Add Edit, Lift, Extract and trim.
- The Pro Tools set has 6 actions. Most Pro Tools keys act on a time range or depend on the edit mode, so they do not match a vidcut action.

**Key sets and panels:**

- The on-screen keyboard. A click on a key and its modifiers shows the commands of that key (p. 122).
- Keys per panel. One key does different things in different panels (pp. 123–124).
  - Only the Transcription window has its own level.

**Keys that move the clip selection:**

- Ctrl+Left, Ctrl+Right, Ctrl+Up and Ctrl+Down move the clip selection in the manual (pp. 825, 900).
- The manual does not tell which clip counts as previous or above.
- To do: measure that rule, then build the four keys.

**Effects Search:**

- Shift+Space is Effects Search on the Cut page (p. 1144).
- vidcut has no Effects Search.
