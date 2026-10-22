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

## What vidcut does

- **The same controls and defaults** on a track: `track.set` with `duck`, its `sources` and those five values, stored only when set. In the Mixer, the track's strip has Ducking, its sources and its Duck Amount, with the times under Advanced. Proposed key, free now: Alt + D, which ducks the selected clip's track under the other audio tracks with the defaults.
- **Driven by speech.** The Ducker follows the source's level, and the manual gives no threshold. vidcut ducks where it hears speech in the sources (Silero VAD, `analysis::non_speech`, [speech](../design/analysis.md#speech)), so room noise, breaths and a bumped microphone don't dip the music. A source not yet measured is measured first, as Ripple Delete Silence does. This is a deviation, recorded when it is built.
- **One mix.** Preview and export hear the same samples (`AudioMix`), and the export's loudness gain comes after the ducking (`loudness_gain`).
- **OpenTimelineIO.** How Resolve writes a Track FX into an OTIO file isn't known, so the ducking goes in vidcut's own metadata, and an OTIO export warns that Resolve won't hear it.
- **Left out:** the clip EQ (p. 745), the Dialogue Leveler (p. 774), compressors and de-essers.

## Open questions

- How the Ducker turns its five values into a gain curve, measured from Resolve's renders, so vidcut's dips sound the same.
- Whether ducking by speech alone misses a use that the level-driven Ducker has, such as sound effects dipping music.
