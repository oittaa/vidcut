# Faster stabilization analysis on the CPU

**Why:**

- With GPU acceleration off, `stabilize::measure` in vidcut-av finds and follows the corners on the threads of the pool ([design](../design/analysis.md#stabilization)).
- The gray picture, the pyramid, the repeat test and `fitted` still run on one thread, between the pairs of frames.
- Measured with `scripts/stabilize-speed.py` and `scripts/usage.py` on 7 clips of 10 s, Perspective, with GPU acceleration off:
  - The CPU path takes 1.6 s to 4.1 s for each clip, on 3 to 7 cores of 16.

**To do:**

- Make the gray picture and the pyramid of the next frame while the current pair is measured.
- Measure again with the same two scripts and the same clips, with GPU acceleration off.
