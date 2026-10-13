# Audio devices on ALSA, checked on real cards

**When:** next time on a Linux desktop with sound cards.

Choosing the audio device (`player.audio_device`, player.md §6) is checked on Windows: all twelve WASAPI outputs of the test machine play as themselves, and the GUI switches to a device chosen from the CLI. On Linux it is checked only on WSL, whose ALSA lists one device (`null`) and whose default won't open. Neither test machine reaches a real card: gmktec's are there, but its user isn't in the `audio` group. So the case the lookup was fixed for, ids like `alsa:sysdefault:CARD=PCH` that cpal's own lookup misses (gotchas.md), has run on no card yet.

On a desktop with cards (PipeWire or PulseAudio serving ALSA):
- `cargo test -p vidcut-av --features audio-tests --test player_device -- --nocapture`: every device listed that opens plays as itself (`plays_on` is the device chosen), and the names are told apart (`each_listed_device_plays_as_itself` prints each one). Devices that won't open at 48 kHz stereo are listed, not failed: check that they are the ones expected (surround, S/PDIF).
- `vidcut doctor`: `audio_outputs` names a card's outputs apart ("…, ALC897 Analog (Front output / input)").
- The GUI: File > Preferences, Player, Sound: pick a card's output and hear the timeline there. With a USB device chosen and unplugged, vidcut opens on the default, saying so (the notice, `player state`'s `audio_error`). `cargo test -p vidcut --features gui-tests --test gui gui_acceptance` switches devices from the CLI.
