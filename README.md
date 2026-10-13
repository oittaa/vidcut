# vidcut

A simple, fully scriptable video cutting tool for Linux and Windows. Behavior follows the Cut page ([docs/cut-page-spec.md](docs/cut-page-spec.md)).

Early development: it works, but expect rough edges.

## What it does

- **Cut-page editing:**
  - a media pool with bins, and a viewer for source clips, the Source Tape and the timeline
  - a dual timeline: an overview of the whole program plus a detail view
  - Smart Insert, Append, Ripple Overwrite, Overwrite, Place On Top
  - split, ripple delete, trimming (resize, roll, slip, J/L cuts)
  - several tracks: B-roll that follows the main story on Track 1, music on audio tracks
  - a mixer with faders, mute, solo and meters
  - snapping, markers, undo
- **Export:** Quick Export and an Export dialog, background jobs, GPU encoding (NVIDIA, AMD, Intel), loudness normalization. Dynamic range, bit depth and HDR metadata follow the footage.
- **Automatic tools:** Ripple Delete Silence, Detect Scene Cuts, the Boring Detector.
- **Media:** stills, image sequences, proxies; GPU decoding (NVDEC; VA-API on Linux, Direct3D 11 on Windows).
- **Interchange:** OpenTimelineIO (`.otio`, `.otioz`, `.otiod`).
- **Everything scriptable:** every GUI action is also a CLI command and a JSON-RPC call on the running app. `vidcut mcp` offers them to AI agents over the Model Context Protocol.
- **Not included:** effects, transitions, titles, color grading.

## Requirements

- FFmpeg 8.0 to 9.0, with its development files.
- Rust from [rustup](https://rustup.rs). `rust-toolchain.toml` pins the version; rustup fetches it on the first build.

**Linux** (Ubuntu 26.04 or similar):

```bash
sudo apt install libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libswscale-dev libswresample-dev libavdevice-dev libasound2-dev libclang-dev rustup
```

The GUI needs a Vulkan driver. Sound goes through ALSA (PipeWire and PulseAudio provide it).

**Windows 10 or 11:** Visual Studio 2022 or its Build Tools with the C++ workload, plus:

```bash
winget install BtbN.FFmpeg.GPL.Shared.9.0
```
```bash
winget install LLVM.LLVM
```

- Set `FFMPEG_DIR` to the FFmpeg folder (the one with `bin`, `include` and `lib`) and put its `bin` on `PATH`.
- If the build doesn't find libclang, set `LIBCLANG_PATH` to `C:\Program Files\LLVM\bin`.
- `scripts/check.sh` runs from Git Bash.

**macOS** (Apple Silicon; open work: [docs/todo/macos.md](docs/todo/macos.md)): Xcode's Command Line Tools and

```bash
brew install ffmpeg
```

## Build and run

```bash
cargo build --release
./target/release/vidcut                     # the GUI (start screen: new or open a project)
./target/release/vidcut -p talk.vidcut      # the GUI with a project (created if missing)
./target/release/vidcut --help              # the CLI
```

On Windows the program is `target\release\vidcut.exe`.

## Where vidcut keeps its files

The docs name the Linux paths.

| Linux | Windows | macOS | Holds |
|---|---|---|---|
| `~/.config/vidcut` | `%APPDATA%\vidcut` | `~/Library/Application Support/vidcut` | preferences, user presets |
| `~/.cache/vidcut` | `%LOCALAPPDATA%\vidcut\cache` | `~/Library/Caches/vidcut` | thumbnails, waveforms, measurements, encoder detection, shaders |
| `~/.local/state/vidcut` | `%LOCALAPPDATA%\vidcut\state` | `~/Library/Application Support/vidcut/state` | undo history, recent projects |
| `$XDG_RUNTIME_DIR/vidcut` | `%TEMP%\vidcut` | `$TMPDIR/vidcut-UID` | running GUIs (live control) |

`XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME` and `XDG_RUNTIME_DIR` override them on every system.

## The GUI

Media Pool on the left with the edit buttons below it, the Viewer on the right, the timeline across the bottom. How editing works: [the spec](docs/cut-page-spec.md). Keys: [spec §11](docs/cut-page-spec.md#11-keyboard-shortcuts-linux-and-windows).

- **Command log** (top right): what you, scripts and agents did, in words. Hover an entry for the request; click it to copy it as a shell command.
- **Preferences** (File > Preferences, Ctrl+,): `~/.config/vidcut/preferences.toml`, also `vidcut preferences`.
- **Live control:** while a GUI has a project open, `vidcut` commands for that project run inside it ([docs/cli.md](docs/cli.md)).

## Try the CLI

```bash
vidcut project create talk.vidcut
vidcut -p talk.vidcut media add cam.mp4
vidcut -p talk.vidcut edit append m1 --range 10s..2m
vidcut -p talk.vidcut export start talk.mp4 --preset youtube-1080p
```

- `--json`: machine-readable output.
- `--headless`: edit the file directly even while a GUI has it open.
- `vidcut schema`: the protocol as JSON Schema.
- Every command, flag and exit code: [docs/cli.md](docs/cli.md).

## Moving a project

Media in the project's folder are stored relative to it. Missing files relink by name and fingerprint (`vidcut media relink`). Timelines move as OpenTimelineIO. Commands: [docs/cli.md](docs/cli.md).

## Use with AI agents

`vidcut mcp` serves every command over the Model Context Protocol. With Claude Code:

```bash
claude mcp add vidcut -- vidcut mcp -p /path/to/talk.vidcut
```

Other clients, and frames as images: [docs/cli.md](docs/cli.md).

## What's next

What vidcut is for and what comes next: [docs/roadmap.md](docs/roadmap.md). Open work: [docs/todo/](docs/todo/), one file per item.

## Contributing

Read [AGENTS.md](AGENTS.md) first; it's written for AI agents but applies to humans too.

## License

MIT ([LICENSE](LICENSE)).

vidcut links the FFmpeg libraries, which are LGPL or GPL depending on the build (Ubuntu's are GPL). If you distribute binaries with them, comply with their licenses.
