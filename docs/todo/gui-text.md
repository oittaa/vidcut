# Short GUI text everywhere

**When:** next.

The files `scripts/check.sh` lists in `long_text_left` still have strings over 12 words ([AGENTS.md](../../AGENTS.md), rule 12): Export and Quick Export, the Media Pool, History, the Transcription window, the timeline's menus and tools, the viewer and the top bar. Shorten their text, move their windows onto `gui/dialog.rs`, and take each file off the list. The top bar's Timeline menu also gets the entries of Timeline Actions.

Some controls need a better form now that they have no tooltips:
- The timeline header's icon buttons, which have no room for words: Timeline Options, Timeline Actions and Edit Actions; Ripple On and Dynamic Trim; a track's Enlarge.
- Skip Non-Speech, xAI's voice threshold: a slider from Keep All to Skip Most.
