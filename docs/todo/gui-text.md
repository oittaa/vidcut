# Short GUI text everywhere

**When:** next.

The files `scripts/check.sh` lists in `long_text_left` still have strings over 12 words ([AGENTS.md](../../AGENTS.md), rule 12): Preferences, Export and Quick Export, the Media Pool, History, the Transcription window, the timeline's menus and tools, the viewer and the top bar. Shorten their text, move their windows onto `gui/dialog.rs`, and take each file off the list. Preferences also gets a sidebar with one page per section, and the top bar's Timeline menu the entries of Timeline Actions.
