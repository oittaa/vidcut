# macOS and Windows

**When:** later.

vidcut runs on Linux. The design keeps the platforms open (libmpv is loaded at runtime, the FFmpeg libraries are in-process), but nothing has been built or tested on macOS or Windows. On macOS, libmpv embeds only through deprecated OpenGL: [our own player](own-player.md) would draw with Metal instead.
