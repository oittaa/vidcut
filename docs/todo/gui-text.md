# Short GUI text everywhere

**When:** next.

Export and Quick Export, the Media Pool's windows, History, the Transcription window, and the timeline's tools move onto `gui/dialog.rs`. The top bar's Timeline menu also gets the entries of Timeline Actions.

Some controls need a better form now that they have no tooltips:
- The timeline header's icon buttons, which have no room for words: Timeline Options, Timeline Actions and Edit Actions; Ripple On and Dynamic Trim; a track's Enlarge.
- Skip Non-Speech, xAI's voice threshold: a slider from Keep All to Skip Most.
