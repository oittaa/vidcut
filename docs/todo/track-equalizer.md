# Track equalizer: the rest

**When:** when a session can use the Fairlight page of Resolve by computer use.

**Why:**

- vidcut has the track equalizer with the filters that the renders measured ([spec §9](../spec.md#9-audio-pp-642-649650-743744-771-773-778779), Track equalizer).
- Some cases use the nearest measured rule. They can sound different from Resolve.
- Some controls of the window are not in vidcut.

**To measure** on the Fairlight page:

- The rule from a typed Q to its whole step. The typed values 0.5 and 1.1 lie halfway between two steps, and went to 18 and 26.
- The range of the Gain fader.
- A Low Pass of Fire at the slopes 6 and 18, in a render of white noise.
- Whether the window shows the slope of a pass filter with Earth, Air and Ice. Their sound does not use it.
- Whether the first click of a double click on the small graph also bypasses the equalizer. In vidcut it does.
- A second sample rate, if a project can have one. The scripting API keeps the timeline at 48 kHz.

**To build:**

- The Gain fader after the bands, once its range is measured.
- The presets of the window.
- Copy and Paste of the "..." menu, also to and from a clip equalizer (p. 745).
- The L, ML, MH and H buttons.
