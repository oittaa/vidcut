# Smart Reframe cases

**When:** Resolve batch SR4 for the freeze frame and the ramp. The script of SR4 is `fixtures/program/scripts/sr4_script.py` in the test data. The Reference Point item waits for a rule that fits its 40 boxes.

Smart Reframe is complex behavior. Its move depends on the content and on the settings. One measurement records only what Resolve did for that input. A rule needs several files that differ in content, subject count, aspect, frame rate and resolution.

The rules of [spec §8.30](../spec.md#830-smart-reframe-pp-735-771772-815-920-956957-1208-measured-in-resolve-studio-212) rest on SR1, SR2 and SR3. These use nine files, crops made from them, and 9:16, 16:9 and 1:1 timelines. Each item below names a line that rests on one clip or on no measurement, and the cases that test it.

## One clip or one file

- **Reference Point.** Now: SR1, SR2 step W5 and SR3 step W3, 42 boxes on eight files.
  - At the frame of the box, the moves of vidcut and Resolve are within 100 pixels on 10 of 12 boxes of SR3.
  - With a loose box, the tracker of vidcut follows the background, on six files. Resolve follows the person on four of them.
  - After the thing in the box is hidden or leaves, Resolve follows other things. Its move then is not its Auto move.
    - Three boxes on one market clip end on the same view. Two boxes on the same vendor give other moves after he leaves.
    - On one clip, the move went away from the woman in the box at once.
  - With two people who move apart, Resolve follows one of them on one clip. With two people who sit still, it stays between them (SR2).
  - This is a fault of vidcut, not a deviation (the owner's decision).
  - One more start of the tracker ran on the 40 boxes of SR2 and SR3: the found person or vehicle that fits the box best.
    - It comes nearer to Resolve on 15 boxes and goes farther on 4. The loose boxes of the film go from 828 and 986 pixels to 360 and 367.
    - The box on two seated people goes from 23 to 480 pixels, because the track takes one person.
    - A loose box on the vlog clip goes from 706 to 867 pixels. A box of 0.95 of the picture on the concert clip goes from 211 to 368.
  - The box around all found things half inside the box comes nearer on 8 boxes of SR3 and goes farther on 4.
  - A start on the found thing only when exactly one found thing is at least half inside the box:
    - On SR3, it comes nearer on 7 boxes and goes farther on 3.
    - The concert box of 0.95 goes from 211 to 368 pixels. A tight box on a hidden boy goes from 51 to 138.
  - A rule may come nearer on some boxes, but it must go farther on none by more than a few pixels. No rule that ran does this. Thus the tracker starts on the box as drawn.
  - The 40 boxes of SR2 W5 and SR3 W3 are the cases for the next rule. Their distances are in `fixtures/program/NOTES.md`, SR2 and SR3.
- **Freeze frame.** Now: one clean clip of SR3. Resolve holds the move still on the frozen frames, as vidcut does.
  - On three clips whose start the freeze of the clip before had cut, the move of Resolve goes on during the freeze.
  - Test (SR4): freezes at frames 40 and 80, on clips 240 frames apart, on five files.
- **Speed ramps.** Now: a ramp from 100 % to 200 % on four clips (SR3). The move follows the source frames that show, as in vidcut.
  - In the 200 % part, Resolve sets as many keyframes for each frame as in the 100 % part. vidcut sets 1.7 times as many there.
  - At a constant 200 %, Resolve sets twice the keyframes of 100 % (SR2). Thus the two results do not agree yet.
  - Test (SR4): a ramp from 100 % to 50 %, on four files.
