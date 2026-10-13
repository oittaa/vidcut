# macOS

**When:** later.

vidcut builds on macOS (Apple Silicon) and passes its unit tests there: CI's `macos` job (`check.yml`), on Homebrew's FFmpeg. The design keeps macOS open (the FFmpeg libraries are in-process, the GUI draws with wgpu, which builds Metal in, the player's sound goes through cpal on Core Audio, live control is a Unix socket), but the GUI hasn't run there yet. What is macOS's own so far: settings and state in `~/Library/Application Support/vidcut`, caches in `~/Library/Caches/vidcut`; `doctor` and the encoder cache name the macOS version and Metal's GPUs; shortcuts read Cmd and Option where Linux and Windows have Ctrl and Alt; jobs run in the utility QoS class, FFmpeg's threads for them in the default one (gui.md §9).

Open:

- **The GUI on a Mac:** a window, the viewer's picture (Metal), sound, the keys, Quick Export; `scripts/check.sh full` after `scripts/fetch-test-media.sh`.
- **File dialogs** open on helper threads (`gui/export.rs`, `Dialogs`); rfd moves AppKit's onto the main thread itself while the app runs. Check that they open.
- **Hardware decoding and encoding:** VideoToolbox, which `hw.rs` doesn't know yet. Decoded frames copied down at first, then shown from GPU memory ([our own player](own-player.md)); exports through `h264_videotoolbox` and `hevc_videotoolbox`.
- **Packaging:** an `.app` bundle with FFmpeg's libraries in it, signed.
