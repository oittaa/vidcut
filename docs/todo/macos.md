# macOS

**When:** later.

vidcut runs on Linux and Windows. The design keeps macOS open (the FFmpeg libraries are in-process, the GUI draws with wgpu, which builds Metal in, the player's sound goes through cpal, live control is a Unix socket), but nothing has been built or tested there. Hardware decoding there is open: [our own player](own-player.md).
