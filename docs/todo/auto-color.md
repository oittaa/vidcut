# Auto Color

**When:** not decided.

vidcut has Auto Color with the legacy method only ([X174](../deviations.md#x174-auto-color-uses-the-legacy-method)).
Two parts are open: the Color tool of the viewer Tools, and a machine-learning method.

**Why:**

- In Resolve, the Color tool of the Cut page viewer Tools has a switch, an Auto Color button and a reset icon ([spec §8.25](../spec.md#825-auto-color-pp-117-771-31283129-measured-in-resolve-studio-212)).
  - Its Auto Color always adds the neural Auto Color tool to node 1. The switch turns that tool off and on. The reset icon removes it.
  - The Color icon of the Tools has a red dot while the clip has the tool. The dot is white while the switch is off.
  - The tool is not Lift and Gain. The legacy method writes Lift and Gain, with no switch.
- Resolve uses the DaVinci Neural Engine by default. Its legacy method is a preference (pp. 117, 3129).

**In the code:**

- `grade.auto_color` writes the Lift and Gain of red, green, blue and the master into a corrector. It has no switch.
- The Auto Color button of the viewer Tools runs the same analysis on node 1. It does not stay lit.
- `auto_color::balance` is the one place of the method. It gives the Lift and Gain from the pixels of the frame.

**To do:**

- A Color tool in the viewer Tools, with a switch, an Auto Color button and Reset, as in Resolve.
  - The switch and Reset need a result that is not in Lift and Gain: an Auto Color tool of the corrector, with its own switch.
  - The owner decides this design before the work starts.
  - It is not measured where the tool of Resolve acts in the node, before or after the primaries.
- A machine-learning method, when one is good enough:
  - It becomes the default in place of the legacy method in `auto_color::balance`.
  - The legacy method then stays as a Use Legacy Auto Color preference, as in Resolve (pp. 117, 3129).
  - The neural result of Resolve is not Lift and Gain. It is an Auto Color tool with the numbers E and W ([spec §8.25](../spec.md#825-auto-color-pp-117-771-31283129-measured-in-resolve-studio-212)).
