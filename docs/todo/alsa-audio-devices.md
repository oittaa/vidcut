# Audio devices on ALSA: a device chosen and unplugged

**When:** next time on a Linux desktop with a USB audio device to unplug.

A check by hand:

1. Choose the USB device in File > Preferences, Player, Sound.
2. Unplug the device.
3. vidcut must play on the default device and tell the user. The status bar shows a notice, and `player state` gives `audio_error`.
4. Plug the device in again. When the device is back and is the chosen device, vidcut must play on it again.
