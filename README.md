# vidcut

A simple, fully scriptable video cutting tool for Linux and Windows. Behavior follows the Cut page ([docs/cut-page-spec.md](docs/cut-page-spec.md)). Early development: expect rough edges.

## What it does

- **Editing:** a media pool with bins; source and timeline viewers; a dual timeline; Smart Insert, Append, Ripple Overwrite, Overwrite, Place On Top; split, ripple delete, trimming (resize, roll, slip, J/L cuts); video and audio tracks; a mixer; snapping, markers, undo.
- **Export:** Quick Export and an Export dialog, background jobs, GPU encoding (NVIDIA, AMD, Intel), loudness normalization. Dynamic range, bit depth and HDR metadata follow the footage.
- **Automatic tools:** Ripple Delete Silence, Detect Scene Cuts, the Boring Detector.
- **Media:** stills, image sequences, proxies, GPU decoding.
- **Interchange:** OpenTimelineIO (`.otio`, `.otioz`, `.otiod`). Clip effects and transitions from another editor are kept and written back.
- **Scriptable:** every GUI action is also a CLI command, a JSON-RPC call on the running app, and an MCP tool for AI agents.
- **Not included:** effects, transitions, titles, color grading.

## Requirements

FFmpeg 8.0 to 9.0 with its development files, and Rust from [rustup](https://rustup.rs) (`rust-toolchain.toml` pins the version).

**Linux** (Ubuntu 26.04 or similar), with a Vulkan driver:

```bash
sudo apt install libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libswscale-dev libswresample-dev libavdevice-dev libasound2-dev libclang-dev rustup
```

**Windows 10 or 11:** Visual Studio 2022 or its Build Tools with the C++ workload, then:

```bash
winget install BtbN.FFmpeg.GPL.Shared.9.0
winget install LLVM.LLVM
```

Set `FFMPEG_DIR` to the FFmpeg folder (with `bin`, `include`, `lib`) and put its `bin` on `PATH`. If libclang isn't found, set `LIBCLANG_PATH` to `C:\Program Files\LLVM\bin`. Scripts run from Git Bash.

**macOS** (Apple Silicon), with `$(brew --prefix rustup)/bin` on `PATH` and `$(brew --prefix ffmpeg-full)/lib/pkgconfig` on `PKG_CONFIG_PATH`:

```bash
brew install ffmpeg-full rustup
```

## Build and run

```bash
cargo build --release
./target/release/vidcut                     # the GUI
./target/release/vidcut -p talk.vidcut      # the GUI with a project (created if missing)
./target/release/vidcut --help              # the CLI
```

## Try the CLI

```bash
vidcut project create talk.vidcut
vidcut -p talk.vidcut media add cam.mp4
vidcut -p talk.vidcut edit append m1 --range 10s..2m
vidcut -p talk.vidcut export start talk.mp4 --preset youtube-1080p
```

While a GUI has the project open, these commands run inside it; `--headless` edits the file directly. Every command, flag and exit code: [docs/cli.md](docs/cli.md). AI agents: `claude mcp add vidcut -- vidcut mcp -p /path/to/talk.vidcut`.

## Where vidcut keeps its files

| Linux | Windows | macOS | Holds |
|---|---|---|---|
| `~/.config/vidcut` | `%APPDATA%\vidcut` | `~/Library/Application Support/vidcut` | preferences, user presets |
| `~/.cache/vidcut` | `%LOCALAPPDATA%\vidcut\cache` | `~/Library/Caches/vidcut` | thumbnails, waveforms, measurements, encoder detection, shaders |
| `~/.local/state/vidcut` | `%LOCALAPPDATA%\vidcut\state` | `~/Library/Application Support/vidcut/state` | undo history, recent projects |
| `$XDG_RUNTIME_DIR/vidcut` | `%TEMP%\vidcut` | `$TMPDIR/vidcut-UID` | running GUIs (live control) |

The `XDG_*` variables override them on every system. The docs name the Linux paths.

## More

- Keys: [spec §11](docs/cut-page-spec.md#11-keyboard-shortcuts-linux-and-windows). Preferences: File > Preferences, or `vidcut preferences`.
- What vidcut is for and what comes next: [docs/roadmap.md](docs/roadmap.md); open work: [docs/todo/](docs/todo/).
- Contributing: read [AGENTS.md](AGENTS.md) first.

## License

MIT ([LICENSE](LICENSE)). vidcut links the FFmpeg libraries, which are LGPL or GPL depending on the build (Ubuntu's are GPL); binaries distributed with them must comply with their licenses.
