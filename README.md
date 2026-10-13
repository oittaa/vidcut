# vidcut

A simple, fully scriptable video cutting tool for Linux and Windows. Behavior follows the Cut page ([docs/cut-page-spec.md](docs/cut-page-spec.md)).

**Status:** early development. It works, but expect rough edges.

## What it does

- **Cut-page editing:**
  - a media pool with bins, and a source/timeline viewer (Source Clip, Source Tape, Timeline)
  - a dual timeline: an overview of the whole program plus a detail view
  - Smart Insert, Append, Ripple Overwrite, Overwrite, Place On Top
  - split, ripple delete, trimming (resize, roll, slip, J/L cuts)
  - several tracks: B-roll that follows the main story on Track 1, music on audio tracks
  - a mixer with faders, mute, solo and meters
  - snapping, markers, undo
- **Export:** Quick Export and an Export dialog (deviation X3), including GPU encoding (NVIDIA, AMD, Intel) and background jobs. Dynamic range, bit depth and HDR metadata follow the footage (spec D44, D46). A loudness target is [docs/decisions/agents.md](docs/decisions/agents.md) §5.
- **Automatic tools:** Ripple Delete Silence, Detect Scene Cuts and the Boring Detector (spec D29–D31).
- **Interchange:** OpenTimelineIO, `.otio` and `.otioz` (deviation X9).
- **Everything scriptable.** Every action in the GUI is also a CLI command and a JSON-RPC call on the running app, so scripts and AI agents can edit with you. `vidcut mcp` offers it all to AI agents over the Model Context Protocol.
- **Deliberately not included:** effects, transitions, titles or color grading.

## Requirements

**Linux:** Ubuntu 26.04 (or similar) with FFmpeg:

```bash
sudo apt install libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libswscale-dev libswresample-dev libavdevice-dev libasound2-dev libclang-dev rustup
```

The GUI draws with Vulkan (the driver any desktop has: Mesa's for AMD and Intel, NVIDIA's own) and plays sound through ALSA, which PipeWire and PulseAudio serve.

**Windows 10 or 11:** Visual Studio 2022 or its Build Tools with the C++ workload, LLVM (for libclang), and an FFmpeg build with its headers and import libraries, such as BtbN's shared GPL build:

```bash
winget install BtbN.FFmpeg.GPL.Shared.9.0
```
```bash
winget install LLVM.LLVM
```

Set `FFMPEG_DIR` to the FFmpeg folder (the one with `bin`, `include` and `lib`) before building, and put its `bin` folder on `PATH`: vidcut loads FFmpeg's DLLs from there. If the build doesn't find libclang, set `LIBCLANG_PATH` to `C:\Program Files\LLVM\bin`. The GUI draws with Vulkan or Direct3D 12 and plays sound through WASAPI. `scripts/check.sh` runs from Git Bash.

**macOS:** with `$(brew --prefix rustup)/bin` on `PATH` and `$(brew --prefix ffmpeg-full)/lib/pkgconfig` on `PKG_CONFIG_PATH`:

```bash
brew install ffmpeg-full rustup
```

Nothing else is needed at run time: vidcut plays video with its own player, on the FFmpeg libraries. The player, proxies and exports decode H.264, HEVC, AV1 and VP9 on the GPU where it can: NVDEC on NVIDIA GPUs, and on AMD and Intel GPUs VA-API on Linux (Mesa's driver, Intel's media driver) and Direct3D 11 on Windows.

vidcut works with FFmpeg 8.0 (Ubuntu 26.04 LTS) up to the newest release, 9.0; both ends are tested.

Rust comes from [rustup](https://rustup.rs). The repository pins its Rust version in `rust-toolchain.toml`, and rustup downloads that toolchain the first time you build.

## Build and run

```bash
cargo build --release
./target/release/vidcut                     # the GUI (start screen: new or open a project)
./target/release/vidcut -p talk.vidcut      # the GUI with a project (created if missing)
./target/release/vidcut --help              # the CLI
```

On Windows the program is `target\release\vidcut.exe`.

## Where vidcut keeps its files

The docs name the Linux places; Windows and macOS have their own:

| Linux | Windows | macOS | Holds |
|---|---|---|---|
| `~/.config/vidcut` | `%APPDATA%\vidcut` | `~/Library/Application Support/vidcut` | preferences, user presets |
| `~/.cache/vidcut` | `%LOCALAPPDATA%\vidcut\cache` | `~/Library/Caches/vidcut` | thumbnails, waveforms, measurements, encoder detection, shaders |
| `~/.local/state/vidcut` | `%LOCALAPPDATA%\vidcut\state` | `~/Library/Application Support/vidcut/state` | undo history, recent projects |
| `$XDG_RUNTIME_DIR/vidcut` | `%TEMP%\vidcut` | `$TMPDIR/vidcut-UID` | running GUIs (live control) |

The `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME` and `XDG_RUNTIME_DIR` variables move them on every system.

## The GUI

Media Pool on the left, with the edit buttons below it; the Viewer on the right; the timeline across the bottom. Editing, tracks, bins and the automatic tools are in the spec.

The **Command log** (top right) lists what you, scripts and agents did, in words. Hover an entry for the request; click it to copy a shell command (deviation X5).

**Preferences** (File > Preferences, Ctrl+,) live in `~/.config/vidcut/preferences.toml` (deviation X12). `vidcut preferences` reads and changes them.

**Keys:** [spec §11](docs/cut-page-spec.md#11-keyboard-shortcuts-linux-and-windows).

While a GUI has the project open, `vidcut` commands for that project run inside it ([docs/cli.md](docs/cli.md), Live control).

## Try the CLI

```bash
vidcut project create talk.vidcut
vidcut -p talk.vidcut media add cam.mp4
vidcut -p talk.vidcut edit append m1 --range 10s..2m
vidcut -p talk.vidcut export start talk.mp4 --preset youtube-1080p
```

Every command, flag and exit code is in [docs/cli.md](docs/cli.md). `vidcut schema` prints the protocol. `--json` is machine-readable output; `--headless` edits the file directly even while a GUI has it open.

## Moving a project

Media kept in the project's folder is stored relative to it, and missing files relink by name and fingerprint (deviation X1). Timelines move as OpenTimelineIO (deviation X9). The commands are in [docs/cli.md](docs/cli.md).

## Use with AI agents

`vidcut mcp` serves every command over the Model Context Protocol (deviation X10, [docs/decisions/agents.md](docs/decisions/agents.md)). With Claude Code: `claude mcp add vidcut -- vidcut mcp -p /path/to/talk.vidcut`. Connecting, and getting frames back as images, is in [docs/cli.md](docs/cli.md).

## What's next

The use cases vidcut is for, and what comes next, are in [docs/roadmap.md](docs/roadmap.md). Open items are in [docs/todo/](docs/todo/), one file each.

## Contributing

Read [AGENTS.md](AGENTS.md) first; it's written for AI agents but applies to humans too. Behavior follows [docs/cut-page-spec.md](docs/cut-page-spec.md), and intentional differences are recorded in [docs/deviations.md](docs/deviations.md).

## License

vidcut's source code is released under the [MIT License](LICENSE).

vidcut links the FFmpeg libraries, which are LGPL or GPL depending on how they were built; Ubuntu's builds are GPL. Binaries you distribute together with them must therefore comply with their licenses. vidcut's own source stays MIT.
