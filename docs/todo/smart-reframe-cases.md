# Smart Reframe cases

**When:** the next Resolve batch.

Smart Reframe is complex behavior. Its move depends on the content and on the settings. One measurement records only what Resolve did for that input. A rule needs several files that differ in content, subject count, aspect, frame rate and resolution.

The rules of [spec §8.30](../spec.md#830-smart-reframe-pp-735-771772-815-920-956957-1208-measured-in-resolve-studio-212) rest on the shots of SR1 and SR2: nine files, crops made from them, and 9:16, 16:9 and 1:1 timelines. Each item below names a line that rests on one clip or on no measurement, and the cases that test it.

## One clip or one file

- **Reference Point.** Now: SR1 and SR2 step W5, 18 boxes on seven files.
  - With a loose box, the tracker of vidcut follows the background, on four of four files. Resolve follows the person on three of them.
  - With a tight box, vidcut follows the person on the walk-out clip of the film (a median of 92 pixels from Resolve). On the crowd clips, it is 189 to 1108 pixels from Resolve.
  - Boxes on two people and on a cyclist give a move within 7 to 49 pixels of Resolve.
  - This is a fault of vidcut, not a deviation (the owner's decision). SR2 shows it on more clips. Thus the tracker is the next fix.
  - Test after the fix: the W5 boxes again, and boxes on the middle frame of a clip.
- **Speed ramps.** Now: 50 % and 200 % on four clips. vidcut smooths on the time of the source.
  - Test: a speed ramp and a freeze frame, on files at 24, 30 and 59.94 fps.
- **Dynamic Zoom boxes.** Now: the usual boxes, with each ease, on four clips.
  - Test: a box that pans and zooms in, and a box that zooms out, on four files.
- **Crop and Fit in other timelines.** Now: Crop and Fit in 1080×1920 timelines only.
  - Test: Crop and Fit with 1280×720, 1920×1080 and 3840×2160 media, in 16:9 and 1:1 timelines.
- **Pan Only and Tilt Only with a rotation.** Now: no case. vidcut frees the axis of the picture, and the move turns with the picture.
  - Test: Pan Only and Tilt Only with rotations of 10° and 90°, on four files.

## Not measured

- **A short gap in the path of the main subject.** With `subject`, a frame where the main path has no box takes the box of the next path of the shot (`reframe::found`). Thus a short gap moves the view to another thing and back. Resolve has no case of this yet.
  - Test: shots where the main person is hidden for a few frames, or leaves and comes back, with other people in the frame. Use four files.
  - The crowd shots of SR2 (P 0, P 250, M 1200, M 2200, C 400) have such gaps. Compare the Auto move of Resolve with `subject` `person` of vidcut.
