# The rows of the File tab

**When:** after a measurement of the items below. Each measurement waits for the owner's OK.

**Why:** [§10.1](../spec.md#101-export-page-pp-41034132-measured-in-resolve-studio-212) gives the measured file names of each row alone. vidcut builds a row or a combination only when its output is measured.

**Runs that settle what vidcut does not build:**

- The frame count of an interlaced clip. No rule explains the measured counts of `interlaced-tff.mov` ([§10.1](../spec.md#101-export-page-pp-41034132-measured-in-resolve-studio-212)). Its EXR renders also have more files than the clip has frames. Runs: an interlaced source with a start timecode, and one at 29.97.
- An EXR of a source whose name ends in a digit. A movie name gets `_` before the count. Run: the EXR names of such a source.
- Place clips in separate folders gives `Event__0001_0001`. The last number is the place of the clip. Run: a clip on V2 with separate folders, to find what the first number is.
- The rows that vidcut refuses together. Runs:
  - Use unique filenames with Source name.
  - EXR with Source name, File suffix, Use unique filenames, and Add source frame count off, one at a time.
- Render speed: what each number of the menu does. The menu has 100 and Maximum. Run: the render time of one clip at 25, 100 and Maximum.
- Preserve 1 with After head on macOS. vidcut refuses After head on a path with no drive letter.
- Replace existing files in place with an image-sequence source.

**Measured, not built:**

- Individual clips render each clip at its own frame rate (p. 4114). vidcut renders each clip at the frame rate of the timeline ([X175](../deviations.md#x175-individual-clips-render-at-the-frame-rate-of-the-timeline)).
  - In the measurement, `interlaced-tff.mov` (1080i 25, 2 s) gave `interlaced-tff00000001.mp4`, and 100 EXR frames numbered 1 to 100.
- The Video tab rows of Individual clips: Render timeline effects and Render at source resolution (p. 4114).
- The Preserved Path row, which shows the folders that the source path gives.

**Rows with no run yet:**

- Use commercial workflow and the rows that it shows (p. 4126).
- Single clip: Filename uses Timeline name, File suffix, File subfolder, Render without timecode, and Start timeline timecode at.
