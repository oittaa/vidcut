# vidcut

A simple, fully scriptable video cutting tool for Linux, modeled on DaVinci Resolve's **Cut page**.

> **Status: early development.**
> - Done:
>   - **M0:** spikes proved frame-exact decoding and mpv preview.
>   - **M1:** the project model, Cut-page edit operations and the headless CLI.
>   - **M2:** frame-exact export with presets, GPU encoding, ranges, per-clip and audio-only output.
>   - **M3:** the GUI shell. It has the media pool, the viewer (Source Clip, Source Tape, Timeline), transport and marks, the edit buttons and an overview timeline. It also has live control: CLI commands and agents drive the running GUI.
> - Next: timeline editing with the mouse (M4). See the milestones below.

## What it will do

- **Cut-page editing:**
  - a media pool and a source/timeline viewer (Source Clip, Source Tape, Timeline)
  - a dual timeline: an overview of the whole program plus a detail view
  - Smart Insert, Append, Ripple Overwrite, Overwrite
  - split, ripple delete, trimming (resize, roll, slip, J/L cuts)
  - snapping, markers, undo
- **Export:**
  - Quick Export presets (H.264/H.265 Master, ProRes 422 HQ, YouTube, Vimeo, TikTok)
  - an Export dialog with codec settings, GPU encoding (NVENC, VAAPI), In–Out range, per-clip export and audio-only export
- **Everything scriptable.** Every action in the GUI is also a CLI command and a JSON-RPC call on the running app, so scripts and AI agents can edit with you.
- **Deliberately not included:** effects, transitions, titles or color grading.

## Requirements

Ubuntu 26.04 (or similar) with FFmpeg 8 and libmpv:

```bash
sudo apt install libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libswscale-dev libswresample-dev libavdevice-dev libclang-dev libmpv2
```

Rust 1.93 or newer (the distro `rustc`/`cargo` packages are fine).

## Build and run

```bash
cargo build --release
./target/release/vidcut                     # the GUI (start screen: new or open a project)
./target/release/vidcut -p talk.vidcut      # the GUI with a project (created if missing)
./target/release/vidcut --help              # the CLI
```

## The GUI

The layout follows the Cut page:
- the Media Pool on the left, with the edit buttons below it;
- the Viewer on the right;
- the timeline across the bottom.

Everything you do is listed in the **Command log** (top right) as the request it sent. Click an entry to copy it as a shell command.

| Keys | |
|---|---|
| Space, J / K / L | play/stop, shuttle (repeat J/L for speed; hold K and tap J/L to step) |
| ← / →, Shift+← / → | one frame, one second |
| ↑ / ↓, Shift+↑ / ↓, Home / End | previous/next edit, previous/next marker, start/end |
| I / O, Alt+X, Shift+I / O | mark In/Out (source, timeline, or the pool clip under the pointer), clear, go to |
| Q | Viewer: Timeline ↔ last source mode |
| = / + / − | type a timecode (absolute or relative); digits fill from the right, so 100 is 1 second |
| F9, Shift+F12, Shift+F10, F10 | Smart Insert, Append, Ripple Overwrite, Overwrite |
| Ctrl+\\, Delete, Alt+Delete | split at the playhead, ripple delete (or the In–Out range), lift |
| Ctrl+C / X / V, Ctrl+Z / Ctrl+Shift+Z | copy, cut, paste at the playhead; undo, redo |
| M, Alt+M, D | add marker, remove marker, disable clip |
| Ctrl+I | import media |

**Live control.** While the GUI has a project open, `vidcut` commands for that project run *inside* it, and the window updates at once. A script or an AI agent can therefore edit alongside you:

```bash
vidcut -p talk.vidcut edit split --at 1m          # shows up in the GUI immediately
vidcut -p talk.vidcut seek 1m                     # moves the GUI's playhead
vidcut -p talk.vidcut screenshot -o shot.png      # what the viewer shows, frame-exact
vidcut -p talk.vidcut log                         # what was done, by whom
```

## Try the CLI

```bash
vidcut new talk.vidcut
vidcut -p talk.vidcut media add cam.mp4 screen.mkv
vidcut -p talk.vidcut edit append m1 --range 10s..2m
vidcut -p talk.vidcut edit split --at 1m
vidcut -p talk.vidcut edit delete V1@1m10s       # ripple delete the clip under 1m10s
vidcut -p talk.vidcut timeline
vidcut -p talk.vidcut undo
vidcut -p talk.vidcut export talk.mp4 --preset youtube-1080p   # GPU-encoded when available
vidcut -p talk.vidcut export talk.flac                          # audio only
vidcut -p talk.vidcut frame --at 1m -o check.png                # look at a frame
```

Every command takes `--json` for machine-readable output. `vidcut schema` lists the full protocol. Add `--headless` to work on the file directly even while a GUI has it open.

## Planned milestones

| | |
|---|---|
| M0 | Scaffold; spikes for mpv preview and frame-exact decoding |
| M1 | Project model, Cut-page edit operations, headless CLI |
| M2 | Export engine (presets, GPU, range, per-clip, audio-only) |
| M3 | GUI shell, viewer, media pool, live control over a socket |
| M4 | Timeline editing (dual timeline, trimming, snapping, markers) |
| M5 | Export UI (Quick Export, Export dialog, jobs), OpenTimelineIO import/export |
| M6 | Multi-track (B-roll, music, Place On Top, mixer) |
| M7 | Agent extras (MCP server, Ripple Delete Silence, analysis) |

## Contributing

Read [AGENTS.md](AGENTS.md) first; it's written for AI agents but applies to humans too. Behavior follows [docs/cut-page-spec.md](docs/cut-page-spec.md), and intentional differences are recorded in [docs/deviations.md](docs/deviations.md).

## License

vidcut's source code is released under the [MIT License](LICENSE).

vidcut links the FFmpeg libraries and loads libmpv at runtime. Both are LGPL or GPL depending on how they were built, and Ubuntu's builds are GPL. Binaries you distribute together with those libraries must therefore comply with their licenses. vidcut's own source stays MIT.
