# Faster stabilization analysis on the CPU

**Why:**

- With GPU acceleration off, `stabilize::measure` in vidcut-av follows the corners of each pair of frames on one CPU thread ([design](../design/analysis.md#stabilization)).
- Measured with `scripts/stabilize-speed.py` and `scripts/usage.py` on 7 clips of 10 s, Perspective, with GPU decoding on:
  - The CPU path takes 3.5 s to 5.5 s for each clip, on about 1 core of 16.
  - The GPU path takes 1.2 s to 3.2 s for each clip.
- The corners of a pair of frames are free of each other. Thus the work can go on all threads.

**To do:**

- Follow the corners of a pair of frames on all CPU threads.
- Make the gray picture and the pyramid of the next frame while the current pair is measured.
- Measure again with the same two scripts and the same clips, with GPU acceleration off.
