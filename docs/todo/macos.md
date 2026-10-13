# macOS

**When:** later.

vidcut builds on macOS (Apple Silicon) and passes its unit tests there: CI's `macos` job (`check.yml`), on Homebrew's FFmpeg. The design keeps macOS open (the FFmpeg libraries are in-process, the GUI draws with wgpu, which builds Metal in, the player's sound goes through cpal on Core Audio, live control is a Unix socket), but the GUI hasn't run there yet. Jobs run in the utility QoS class (gui.md §9).

Open:

- **The GUI on a Mac:** a window, the viewer's picture (Metal), sound, the keys (Cmd where Linux and Windows have Ctrl), Quick Export; `scripts/check.sh full`.
- **File dialogs** open on helper threads (`gui/export.rs`, `Dialogs`); AppKit's want the main thread. If they fail there, they move to it.
- **System info:** `system.rs` reads the system's version and its GPUs from `/proc` and `/sys`, Linux's. On macOS both are empty: `doctor` shows neither, and the encoder cache's key lacks them.
- **Where files live:** `dirs.rs` puts settings in `~/.config` and caches in `~/.cache` on macOS too; macOS keeps them in `~/Library/Application Support` and `~/Library/Caches`.
- **Live control's socket:** macOS takes 104 bytes for a socket's path, and without `XDG_RUNTIME_DIR` the socket is in the temp folder, which is long there (`/var/folders/…/T/`).
- **Hardware decoding and encoding:** VideoToolbox, which `hw.rs` doesn't know yet. Decoded frames copied down at first, then shown from GPU memory ([our own player](own-player.md)); exports through `h264_videotoolbox` and `hevc_videotoolbox`.
- **Test media:** `scripts/fetch-test-media.sh` needs `sha256sum` and Python 3.11 (`tomllib`); check what a Mac has.
- **Packaging:** an `.app` bundle with FFmpeg's libraries in it, signed.
