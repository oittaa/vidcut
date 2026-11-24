# The rows of the File tab

**When:** after a measurement of the file names below. The measurement waits for the owner's OK.

**Why:** the measurements give the rows of the File tab and their defaults ([§10.1](../spec.md#101-export-page-pp-41034132-measured-in-resolve-studio-212)). They do not give the file names that each row gives. vidcut adds a row only when its output is measured.

**Setup:**

- A timeline with three clips of 1 s from two source files in two folders: `A/clip_a.mov` two times and `B/clip_b.mov` one time.
- The clips keep their names.
- Render Individual clips with MP4 H.264, and then with EXR, to an empty folder.
- After each run, list the written files with their paths.

**Runs** (one change from the defaults in each; the numbers are those of the measurement plan):

- Run 1, the defaults: Custom name "Untitled", Add source frame count on, the other rows off. The file names.
- Run 2, Filename uses Source name. The file names.
- Run 3, File suffix "_x". The file names.
- Run 4, Use unique filenames on. The file names.
- Run 5, Add source frame count to filename off. The file names.
- Run 6, File subfolder "sub". The paths.
- Run 7, Place clips in separate folders on. The paths.
- Run 8, Preserve source directory levels 1 with After head, and then 1 with From tail. The paths.
- Run 9, EXR only: Use 4 digits, and Each clip starts at frame 1 on. The name of the first frame file of each clip.
- Run 10, the clip that is used two times. Do its two files get different names?
- Run 12, Replace existing files in place, on copies of the source files. What does it write?
- Run 13, Render speed. The choices of the menu.

**Rows with no run yet:**

- Use commercial workflow and the rows that it shows (p. 4126).
- Single clip: Filename uses Timeline name, File suffix, File subfolder, Render without timecode, and Start timeline timecode at.
