# Meters that tap the player's output

**When:** later.

The meters (spec D28) read the media waveforms at the playhead, with gains, mute and solo applied: they show the edit's levels, not what the player sends to the audio device. mpv's client API doesn't hand the session thread those samples.
