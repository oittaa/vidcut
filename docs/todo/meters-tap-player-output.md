# Meters that tap the player's output

**When:** later.

The meters (spec D28) read the media waveforms at the playhead, with gains, mute and solo applied: they show the edit's levels, not what the player sends to the audio device. The player engine mixes those samples itself now (player.md §4): its audio thread could keep each lane's and the mix's peaks for the meters, moving with what is heard.
