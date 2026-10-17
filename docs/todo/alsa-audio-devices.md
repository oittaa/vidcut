# Audio devices on ALSA: a device chosen and unplugged

**When:** next time on a Linux desktop with a USB audio device to unplug.

`vidcut doctor` names a card's outputs apart, and the GUI plays on the output chosen in File > Preferences, Player, Sound (checked on a desktop with PipeWire serving ALSA). Left to check: with a USB device chosen and then unplugged, vidcut opens on the default, saying so (the status bar's notice, and `player state`'s `audio_error`), and plays on the device again once it is back and chosen.
