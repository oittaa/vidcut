# Faster stabilization analysis

**When:** after the quality of the stabilization is good on the test clips. Speed comes second.

**Why:**

- `stabilize::measure` in vidcut-av follows the corners of each pair of frames on one CPU thread ([design](../design/analysis.md#stabilization)).
- Measured with `scripts/stabilize-speed.py` and `scripts/usage.py` on 7 clips of 10 s, Perspective:
  - vidcut takes 3.9 s to 5.4 s for each clip, on about 1 core of 16. The GPU only decodes.
  - Resolve Studio 21.2 takes 1.0 s to 2.7 s, mostly on the GPU (3D engine 56% to 93% busy).
  - The CPU time is about the same: 29 s for vidcut, 25 s for Resolve, for all 7 clips.
- Thus the work is not larger. It only runs on one thread.

**To do:**

- Follow the corners of a pair of frames on all CPU threads. Each corner is free of the others.
- Make the gray picture and the pyramid of the next frame while the current pair is measured.
- Then measure on the GPU (AGENTS.md rule 9): the pyramid, the corners and the Lucas–Kanade steps. Keep the CPU path for a GPU that gives wrong results.
- Measure again with the same two scripts and the same clips.
