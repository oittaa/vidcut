# Audio devices on ALSA, checked on real cards

**When:** next time on a Linux desktop with sound cards.

Choosing the audio device (`player.audio_device`, player.md §6) hasn't run on a real ALSA card, including the ids cpal's own lookup misses (`alsa:sysdefault:CARD=PCH`, gotchas.md). WSL has no cards; gmktec has, but its user isn't in the `audio` group.

On a desktop with cards (PipeWire or PulseAudio serving ALSA):
- `cargo test -p vidcut-av --features audio-tests --test player_device -- --nocapture`: every listed device that opens plays as itself (`plays_on` is the device chosen), and the names are told apart. Devices that won't open at 48 kHz stereo are listed, not failed: check they are the expected ones (surround, S/PDIF).
- `vidcut doctor`: `audio_outputs` tells a card's outputs apart ("…, ALC897 Analog (Front output / input)").
- The GUI: File > Preferences, Player, Sound: pick a card's output and hear the timeline there. Unplug a chosen USB device: vidcut plays on the default and says so (the notice; `player state`'s `audio_error`). `cargo test -p vidcut --features gui-tests --test gui gui_acceptance` switches devices from the CLI.
