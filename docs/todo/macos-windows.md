# macOS and Windows

**When:** later.

vidcut runs on Linux. The design keeps the platforms open (the FFmpeg libraries are in-process, the GUI draws with wgpu, which builds Metal and DX12 in, and the player's sound goes through cpal), but nothing has been built or tested on macOS or Windows. Hardware decoding there is open: [our own player](own-player.md).
