# vidcut

A simple, fully scriptable video cutting tool for Linux, Windows and macOS. It behaves like a Cut page ([spec](docs/cut-page-spec.md)), and everything you can do in its window you can also do from a script or an AI agent.

**Status:** early development. It works, but expect rough edges.

## What it does

- **Editing:** a media pool with bins; a source and timeline viewer; the dual timeline; Smart Insert, Append, Ripple Overwrite, Overwrite, Place On Top; split, ripple delete, trimming, the Trim Editor and Dynamic Trim; clip speed; dissolves and fades; several video and audio tracks; a mixer; snapping, markers and undo.
- **Export:** Quick Export and an Export dialog, GPU encoding (NVIDIA, AMD, Intel, Apple), background jobs, HDR that follows the footage, loudness normalization.
- **Automatic tools:** Ripple Delete Silence, Detect Scene Cuts, the Boring Detector.
- **Interchange:** OpenTimelineIO (`.otio`, `.otioz`, `.otiod`), which also open as projects.
- **Scriptable:** every action is a CLI command, a JSON-RPC call on the running app and an MCP tool.
- **Not included:** effects, titles, color grading, transitions other than dissolves. Those in another editor's OTIO file are kept and written back.

Where vidcut differs from the Cut page: [docs/deviations.md](docs/deviations.md).

## Requirements

**Linux:** Ubuntu 26.04 (or similar) with FFmpeg:

```bash
sudo apt install libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libswscale-dev libswresample-dev libavdevice-dev libasound2-dev libclang-dev rustup
```

**Windows 10 or 11:** Visual Studio 2022 or its Build Tools with the C++ workload, LLVM, and an FFmpeg build with headers and import libraries, such as BtbN's shared GPL build:

```bash
winget install BtbN.FFmpeg.GPL.Shared.9.0
```
```bash
winget install LLVM.LLVM
```

Set `FFMPEG_DIR` to the FFmpeg folder (with `bin`, `include` and `lib`) and put its `bin` on `PATH`. If the build doesn't find libclang, set `LIBCLANG_PATH` to `C:\Program Files\LLVM\bin`.

**macOS:** with `$(brew --prefix rustup)/bin` on `PATH` and `$(brew --prefix ffmpeg-full)/lib/pkgconfig` on `PKG_CONFIG_PATH`:

```bash
brew install ffmpeg-full rustup
```

On a Mac the full test gate and an app bundle are still open ([todo](docs/todo/macos.md)).

vidcut works with FFmpeg 8.0 up to 9.0, decodes on the GPU where it can (NVDEC, VA-API, Direct3D 11, VideoToolbox), and needs nothing else at run time. The GUI draws with Vulkan, Direct3D 12 or Metal and plays sound through ALSA (served by PipeWire or PulseAudio), WASAPI or Core Audio. `rust-toolchain.toml` pins the Rust version; rustup fetches it on the first build.

## Build and run

```bash
make doctor                 # what a build still needs, then the GPUs and encoders
make                        # build
make run                    # the GUI
make run P=talk.vidcut      # the GUI with a project (created if missing)
make install                # vidcut into ~/.local/bin
```

`make help` lists the rest. On Windows, `cargo build --release` builds `target\release\vidcut.exe`.

## Try the CLI

```bash
vidcut project create talk.vidcut
vidcut -p talk.vidcut media add cam.mp4
vidcut -p talk.vidcut edit append m1 --range 10s..2m
vidcut -p talk.vidcut export start talk.mp4 --preset youtube-1080p
```

`vidcut --help` lists every command, and `vidcut <command> --help` its flags. While a GUI has the project open, these commands run inside it. [docs/cli.md](docs/cli.md) covers times, JSON output, exit codes and scripting.

With Claude Code: `claude mcp add vidcut -- vidcut mcp -p /path/to/talk.vidcut`.

## Files

| Linux | Windows | macOS | Holds |
|---|---|---|---|
| `~/.config/vidcut` | `%APPDATA%\vidcut` | `~/Library/Application Support/vidcut` | preferences, presets, themes |
| `~/.cache/vidcut` | `%LOCALAPPDATA%\vidcut\cache` | `~/Library/Caches/vidcut` | thumbnails, waveforms, measurements, encoder detection, shaders |
| `~/.local/state/vidcut` | `%LOCALAPPDATA%\vidcut\state` | `~/Library/Application Support/vidcut/state` | undo history, recent projects |
| `$XDG_RUNTIME_DIR/vidcut` | `%TEMP%\vidcut` | `$TMPDIR/vidcut-UID` | running GUIs |

`XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME` and `XDG_RUNTIME_DIR` move them on every system. `vidcut reset` (or File > Preferences, Reset vidcut…) clears the first three back to a fresh install's state while no other vidcut GUI runs; your presets and themes stay unless you ask (`--presets-and-themes`), and so does a proxy folder you set. `--dry-run` lists what would go. Preferences are in File > Preferences or `vidcut preferences`; keys in Help (F1); themes are TOML files ([starship.toml](crates/vidcut/themes/starship.toml) explains every key).

## Contributing

Read [AGENTS.md](AGENTS.md) first: the rules, the quality gates and the docs. How vidcut is built is in [docs/design/](docs/design/README.md). What comes next is in [docs/roadmap.md](docs/roadmap.md) and [docs/todo/](docs/todo/).

## License

vidcut's source code is MIT ([LICENSE](LICENSE)). The fonts it carries, Geist, Geist Mono and Space Grotesk, are under the SIL Open Font License 1.1. vidcut links the FFmpeg libraries, which are LGPL or GPL depending on how they were built; binaries distributed with them must follow their licenses.
