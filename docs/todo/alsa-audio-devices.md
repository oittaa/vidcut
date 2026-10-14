# Audio devices on ALSA, checked on real cards

**When:** next time on a Linux desktop with sound cards.

Check choosing the audio device (`player.audio_device`, player.md §6) on real ALSA cards, with ids like `alsa:sysdefault:CARD=PCH` that cpal's own lookup misses (gotchas.md).

On a desktop with cards (PipeWire or PulseAudio serving ALSA):
- `vidcut doctor`: `audio_outputs` names a card's outputs apart ("…, ALC897 Analog (Front output / input)").
- The GUI: File > Preferences, Player, Sound: pick a card's output and hear the timeline there. With a USB device chosen and unplugged, vidcut opens on the default, saying so (the notice, `player state`'s `audio_error`).
