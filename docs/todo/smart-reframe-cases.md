# Smart Reframe cases

**When:** the window steps of Resolve batch SR3 (W1 to W3). Its script steps ran (SR3 in `fixtures/program/NOTES.md`).

Smart Reframe is complex behavior. Its move depends on the content and on the settings. One measurement records only what Resolve did for that input. A rule needs several files that differ in content, subject count, aspect, frame rate and resolution.

The rules of [spec §8.30](../spec.md#830-smart-reframe-pp-735-771772-815-920-956957-1208-measured-in-resolve-studio-212) rest on SR1, SR2 and the script steps of SR3. These use nine files, crops made from them, and 9:16, 16:9 and 1:1 timelines. Each item below names a line that rests on one clip or on no measurement, and the cases that test it.

## One clip or one file

- **Reference Point.** Now: SR1 and SR2 step W5, 18 boxes on seven files.
  - With a loose box, the tracker of vidcut follows the background, on four of four files. Resolve follows the person on three of them.
  - With a tight box, vidcut follows the person on the walk-out clip of the film (a median of 92 pixels from Resolve). On the crowd clips, it is 189 to 1108 pixels from Resolve.
  - Boxes on two people and on a cyclist give a move within 7 to 49 pixels of Resolve.
  - This is a fault of vidcut, not a deviation (the owner's decision). SR2 shows it on more clips.
  - Four other starts of the tracker ran on the 16 boxes of W5. None of them comes nearer to Resolve on all boxes:
    - The found person or vehicle that fits the box best. The loose boxes of the film and of the market come to 360 and 106 pixels (before: 828 and 463). The box on two people goes from 23 to 480 pixels, because the track takes one person.
    - The box around all people and vehicles that are half inside the box: nearly the same. With a third inside, the box on two people gives 115 pixels, and three other boxes are farther.
    - The path of the found person instead of the tracker: nearer on four boxes, farther on nine.
    - No box on frames with a low score of the tracker: no change. After the man of the film leaves, the tracker takes another thing, with a score of about 0.5.
  - Thus the fix needs the rule by which Resolve picks the subject of a box. SR3 step W3 tests it with these boxes:
    - Four sizes of box on one person, on two files.
    - Boxes on things that are not people.
    - People who are hidden for a moment.
    - A still person, with another person who walks through the box.
    - Two people who move apart.
- **Speed ramps.** Now: 50 % and 200 % on four clips. vidcut smooths on the time of the source.
  - Test (SR3 step W2): a speed ramp and a freeze frame, on files at 24, 25, 30 and 59.94 fps.
- **Pan Only and Tilt Only with a rotation.** Now: no case. vidcut frees the axis of the picture, and the move turns with the picture.
  - Test (SR3 step W1): Pan Only and Tilt Only with rotations of 10° and 90°, on four files each.

## Not a case for Resolve

- **Other things at the ends of the main path.** With `subject`, a frame where the main path has no box takes the box of the next path of the shot (`reframe::found`).
  - The finders keep a thing on its path through a gap of up to a second. On the 21 shots of SR2 (9 files), the view never goes to another thing and back.
  - On 8 of the 21 shots, the main path starts after the shot starts or ends before the shot ends. There the view follows other people, for up to 41 frames.
  - On M 2200, three people come one after the other in the first 19 frames. Then the main person comes.
  - Resolve cannot test this, because it has no `subject`. On the crowd shots, the `subject` move is a median of 161 to 1624 pixels from the Auto move of Resolve.
  - To decide (the owner): on those frames, follow the next thing, or keep the nearest place of the main thing.
