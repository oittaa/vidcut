# Ducking

**When:** next, first in [the roadmap](../roadmap.md#what-comes-next). Music under a voice-over or a talk is the most common mix, and vidcut already knows where the speech is.

## What the Cut page does

The Ducker Track FX (pp. 778–779, and in the Inspector's Audio tab, p. 744) lowers a target track, such as music, by the level of one or more source tracks, such as dialogue. It changes the level only, and compresses nothing. A graph shows the source's level against the ducking.

| Control | Default |
|---|---|
| Duck Amount | 2.7 dB; 2 to 5 dB works best most of the time |
| Lookahead | 15 ms |
| Rise Time | 10 ms |
| Hold | 150 ms |
| Recovery Time | 750 ms |

### The gain curve, measured

These facts come from renders of DaVinci Resolve Studio 21.1.1 on Windows, at 48 kHz.

- The source track has 3 kHz tone bursts of 50 ms to 2 s, at -6 to -60 dBFS peak.
- The target track has a steady 220 Hz tone at -12 dBFS.
- The gain is the rendered target divided by the input target, after the source is subtracted.

These facts are the same for each setting and each track type:

- The dip is the Duck Amount exactly: 2.700 dB at 2.7, and 10.000 dB at 10.
- The source level does not change the dip. Bursts at -6, -12, -24 and -36 dBFS give the same curve.
- Bursts at -48 and -60 dBFS give no dip. Thus the Ducker has a fixed threshold between -48 and -36 dBFS peak.
- A burst of 50 ms gives the full dip, and the same recovery as a burst of 2 s.
- A gap of 100 ms between two bursts keeps the full dip, with a Hold of 150 ms.
- The gain changes in steps of 16 samples.
- Each fall starts at a whole multiple of 10 ms from the start of the timeline.
- The render has no delay. The source and the target stay aligned with the timeline.

The recovery, and the fall with a Rise Time of 200 ms, are half cosines in dB. For a Duck Amount of A dB, a change of length T, and the time t from the start of the change:

- Fall: gain = -A × (1 - cos(π t / T)) / 2 dB.
- Recovery: gain = -A × (1 + cos(π t / T)) / 2 dB.

#### A mono target track

The times are in ms. A fall time is from the start of the source. A recovery time is from the end of the source. The Duck Amount is 10 dB.

| Lookahead | Rise Time | Hold | Recovery Time | Fall starts | Full dip | Recovery starts | Recovery ends |
|---|---|---|---|---|---|---|---|
| 15 | 10 | 150 | 750 | -40 | -6 | +151 | +898 |
| 15 | 200 | 150 | 750 | -230 | -30 | +171 | +918 |
| 100 | 10 | 500 | 1000 | -120 | -86 | +512 | +1506 |

"Full dip" is within 0.01 dB of the Duck Amount. "Recovery ends" is within 0.001 dB of 0 dB.

- The fall starts Lookahead + Rise Time + 10 to 15 ms before the source.
- With a Rise Time of 10 ms, the fall is a one-pole curve on the linear gain, not a half cosine.
- In each 16 samples of that fall, the gain closes 7.0 % of the remaining distance. Thus its time constant is 4.6 ms.
- With a Rise Time of 200 ms, the fall is a half cosine of 202 ms. The fit error is 0.04 dB rms.
- The recovery starts Hold + 1 to 21 ms after the source ends, for each Lookahead and Rise Time measured.
- The recovery is a half cosine that lasts the Recovery Time: 750 ms and 1000 ms.
- The half cosine fits the recovery to 0.011 dB rms or better. A line in dB fits to 0.17 dB rms at best.

#### A stereo target track

The target has a sine on the left and a cosine on the right, so each channel's gain is known. The times are for the right channel.

| Duck Amount | Lookahead | Rise Time | Hold | Recovery Time | Fall starts | Full dip | Recovery starts | Recovery ends |
|---|---|---|---|---|---|---|---|---|
| 2.7 | 15 | 10 | 150 | 750 | -40 | -30 | +60 | +430 |
| 10 | 15 | 10 | 150 | 750 | -40 | -26 | +60 | +430 |
| 10 | 15 | 200 | 150 | 750 | -230 | -130 | -29 | +348 |
| 10 | 15 | 10 | 500 | 750 | -40 | -26 | +231 | +608 |
| 10 | 15 | 10 | 150 | 1000 | -40 | -26 | +60 | +556 |
| 10 | 100 | 10 | 150 | 750 | -120 | -106 | +20 | +390 |

- The fall starts at the same time as on a mono track.
- Rise Time, Hold and Recovery Time act at half their value.
- The fall with a Rise Time of 200 ms lasts 100 ms. The recovery lasts 376 ms at 750, and 500 ms at 1000.
- The recovery starts (Hold - Lookahead - Rise Time) / 2 after the source ends, to within 7 ms.
- The recovery, and the fall with a Rise Time of 200 ms, are half cosines, as on a mono track.
- The two channels get different gains while the gain changes.
- At 10 dB with a Rise Time of 10 ms, the right channel falls 8.2 dB in the first 16 samples. The left channel falls 0.4 dB.
- In a fall, the gain of each channel jumps at the 10 ms steps.
- In the recovery, the left channel is 1 to 9 ms later than the right, and 0.2 dB apart at most.
- The cause of these differences is not known.

#### The controls

- The Inspector calls the Duck Amount "Duck Level".
- The Recovery Time is 1000 ms at most. A typed 2000 becomes 1000.
- A Rise Time of 200 ms, a Hold of 500 ms and a Lookahead of 100 ms stay as typed. The other limits are not measured.
- On a timeline with two audio tracks, Source 1 of a new Ducker is the other track. With three tracks, Source 1 is empty.

## What vidcut does

- **The same controls and defaults** on a track: `track.set` with `duck`, its `sources` and those five values, stored only when set. In the Mixer, the track's strip has Ducking, its sources and its Duck Amount, with the times under Advanced. Proposed key, free now: Alt + D, which ducks the selected clip's track under the other audio tracks with the defaults.
- **Driven by speech.** vidcut ducks where it hears speech in the sources (Silero VAD, `analysis::non_speech`, [speech](../design/analysis.md#speech)).
  - The Ducker dips by the full Duck Amount where the source is above a fixed level ([measured](#the-gain-curve-measured)).
  - In vidcut, room noise, breaths and a bumped microphone don't dip the music.
  - A source not yet measured is measured first, as Ripple Delete Silence does.
  - This is a deviation. It is recorded when it is built.
- **One mix.** Preview and export hear the same samples (`AudioMix`), and the export's loudness gain comes after the ducking (`loudness_gain`).
- **OpenTimelineIO.** How Resolve writes a Track FX into an OTIO file isn't known, so the ducking goes in vidcut's own metadata, and an OTIO export warns that Resolve won't hear it.
- **Left out:** the clip EQ (p. 745), the Dialogue Leveler (p. 774), compressors and de-essers.

## Open questions

- Which timing vidcut copies on a stereo track. The mono timing matches the controls. The stereo timing acts at half the times, and moves the channels apart.
- Whether ducking by speech alone misses a use that the level-driven Ducker has, such as sound effects dipping music.
