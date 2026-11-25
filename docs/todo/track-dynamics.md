# Track Dynamics: the rest

**When:** when a session can use the Fairlight page of Resolve by computer use.

**Why:**

- vidcut has the track Dynamics with the processor that the renders measured ([spec §9](../spec.md#9-audio-pp-642-649650-743744-771-773-778779), Track Dynamics).
- Some cases use the nearest measured rule. They can sound different from Resolve.
- Some controls of the window are not in vidcut.

**To measure** on the Fairlight page:

- Mix at 0, 25, 50, 75 and 100, with two settings and two signals: run R17 (`scripts/fl_mix_cases.py` of the fixtures).
  - The renders of Mix 50 and 100 give the input the gain (Mix/100)². The manual says that Mix goes from compressed to not compressed.
  - If R17 gives the same rule, vidcut takes it (`dynamics::mixed`).
- A second sample rate, if a project can have one. The detector of vidcut runs once in each 3 samples at all rates.
  - Resolve can also run its detector at a fixed 16 kHz. The renders at 48 kHz cannot show which rule it uses.
- The rule from the position of a control to its value. The renders give the value only at some positions.
- The Knee at thresholds other than −30 dB, and at ratios far from 4.
- The renders of the Compressor at the ratios 1.2 and 1.6 are 0.1 dB lower than the processor. The cause is not known.
- A render that starts after the start of the timeline. The phase of the detector runs is known only from the start.
- The Expander at the Attack 0.50 ms, and signals that clip inside the modules.
- The preset values `DYN_TYPE`, `DYN_HPF_FREQ` and `DYN_LINK_IN`. The window shows no control for them.
- What the graph of the window shows, and its moving balls.
- Whether the first click of a double click on the small graph also bypasses the Dynamics. In vidcut it does.

**To build:**

- The presets of the window.
- The Sidechain of the Compressor, with On, Listen and Source.
- The moving level on the graph of the window.
