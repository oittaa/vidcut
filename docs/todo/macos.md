# macOS

**When:** later.

vidcut builds on macOS (Apple Silicon) and passes its unit tests there: CI's `macos` job (`check.yml`), on Homebrew's FFmpeg. The GUI works on a MacBook Air (M4, macOS 27): the window, the viewer's picture on Metal, sound through Core Audio, the file dialogs, the Cmd keys, Quick Export, and reopening a project. What is macOS's own so far: settings and state in `~/Library/Application Support/vidcut`, caches in `~/Library/Caches/vidcut`; `doctor` and the encoder cache name the macOS version and Metal's GPUs; shortcuts read Cmd and Option where Linux and Windows have Ctrl and Alt; Show in Folder selects the file in Finder; jobs run in the utility QoS class, FFmpeg's threads for them in the default one (gui.md §9).

Open:

- **The full tests on a Mac:** `scripts/check.sh full` after `scripts/fetch-test-media.sh`.
- **Hardware decoding and encoding:** VideoToolbox, which `hw.rs` doesn't know yet. Decoded frames copied down at first, then shown from GPU memory ([our own player](own-player.md)); exports through `h264_videotoolbox` and `hevc_videotoolbox`.
- **Packaging:** an `.app` bundle with FFmpeg's libraries in it, signed.
- **A trackpad swipe** moving the playhead: [trackpad-swipe.md](trackpad-swipe.md).
