# Smart Reframe cases

**When:** the next Resolve batch.

Smart Reframe is complex behavior. Its move depends on the content and on the settings. One measurement records only what Resolve did for that input. A rule needs several files that differ in content, subject count, aspect, frame rate and resolution.

The rules of [spec §8.30](../spec.md#830-smart-reframe-pp-735-771772-815-920-956957-1208-measured-in-resolve-studio-212) rest on shots of two files. Some lines rest on one clip or one file. Each item below names what a line rests on now, and the cases that test it.

- **Files for each case:** the press conference (59.94 fps, 3840×2160), the film (24 fps, 1280×534), and at least two more files. Use one file at 25 or 30 fps and one with two or more people. Use one 4:3 file.
- **Frame shapes:** 9:16 and 16:9 timelines, and one 1:1 timeline.
- **Prepared:** section SR2 of `fixtures/program/NOTES.md` in the test data gives the files, the timelines and the steps. The scripts are `sr2_render.py`, `sr2_make.py` and `sr2_script.py`. The batch takes about 60 minutes of Resolve.

## One clip or one file

- **No room, from a start that is not 0.** Now: Auto from Tilt 50 (one clip), Tilt Only from Tilt 50 (one clip), Pan Only from Pan 100 (one clip). vidcut puts that axis at the middle.
  - Test: Auto, Pan Only and Tilt Only from Pan 100, Pan −150 and Tilt 50, on four files, in 9:16 and 16:9 timelines.
- **The axis that Pan Only or Tilt Only does not move.** Now: no case with a start that is not 0. vidcut keeps its value at each frame.
  - Test: Tilt Only from Pan 100, and Pan Only from Tilt 50 with a zoom of 1.5 (room on both axes), on four files.
- **A start that is not 0, on an axis with room.** Now: Pan 100 on one clip.
  - Test: Pan 100, Pan −150, and Position keyframes before Reframe, on four files.
- **Crop and Fit.** Now: one 3840×2160 clip at Crop, one 1280×534 clip at Fit.
  - Test: Crop and Fit with 1280×720, 1920×1080 and 3840×2160 media, in 9:16 and 16:9 timelines.
- **The head near the top with a zoom.** Now: four shots of the press conference.
  - Test: a zoom of 1.5 and 2 on shots of one person, of two people and of a wide scene, on four files.
- **Rotation.** Now: 10° on one clip.
  - Test: −30°, 10° and 90°, on four files.
- **Dynamic Zoom.** Now: the usual boxes on one clip.
  - Test: a box that pans and zooms in, a box that zooms out, and each ease, on four files.
- **Speed.** Now: 50 % and 200 % on one clip of the film.
  - Test: 50 %, 200 % and a speed ramp, on files at 24, 30 and 59.94 fps.
- **Reference Point.** Now: one clip with the box moved to the right, and one walk-out clip with a large box.
  - On the walk-out clip, the tracker of vidcut follows the background in the large box. After the man leaves, the track moves to another thing. Resolve follows the man and then holds.
  - Test: a tight box and a large box on subjects that walk out, on four files. Use a box on two people, a box on a thing that is not a person, and boxes on the first, middle and last frame.
- **Shot changes.** Now: seven clips of the film.
  - Test: clips with a cut from the press conference (camera changes) and from two more files, at different frame rates.
- **The size of the tolerance.** Now: the same keyframes at 1080×1920 and 2160×3840, on 9:16 timelines only. vidcut uses a frame 1080 pixels on its short side.
  - Test: 9:16 clips that move, at 1920×1080 and 3840×2160, and one shot at 1080×1080. That shows if the tolerance follows the short side, the long side or the width.
- **The first box.** Now: one 9:16 timeline.
  - Test: the first box on a 16:9 and a 1:1 timeline.

## Two files

- **The defaults** `smooth` 0.5 s, `tolerance` 4 pixels, and the mean place of the saliency map. Now: a fit on 16 shots of two files, all in 9:16 timelines at 24 fps.
  - Test: a fit on shots of four or more files, in 9:16 and 16:9 timelines, with one and several people.
- **The point at the middle of the frame with a zoom.** Now: 14 shots of two files at a zoom of 1.5.
  - Test: zooms of 1.5 and 2 on four files.
