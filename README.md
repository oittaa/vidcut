# vidcut

A simple, fully scriptable video cutting tool for Linux, modeled on DaVinci Resolve's **Cut page**.

> **Status: early development.**
> - Done:
>   - **M0:** spikes proved frame-exact decoding and mpv preview.
>   - **M1:** the project model, Cut-page edit operations and the headless CLI. Timelines can be built and edited from the command line.
> - Not yet: export and the GUI. See the milestones below.

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
./target/release/vidcut --help     # CLI (the GUI arrives in M3)
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
```

Every command takes `--json` for machine-readable output. `vidcut schema` lists the full protocol.

## Planned milestones

| | |
|---|---|
| M0 | Scaffold; spikes for mpv preview and frame-exact decoding |
| M1 | Project model, Cut-page edit operations, headless CLI |
| M2 | Export engine (presets, GPU, range, per-clip, audio-only) |
| M3 | GUI shell, viewer, media pool, live control over a socket |
| M4 | Timeline editing (dual timeline, trimming, snapping, markers) |
| M5 | Export UI (Quick Export, Export dialog, jobs) |
| M6 | Multi-track (B-roll, music, Place On Top, mixer) |
| M7 | Agent extras (MCP server, Ripple Delete Silence, analysis) |

## Contributing

Read [AGENTS.md](AGENTS.md) first; it's written for AI agents but applies to humans too. Behavior follows [docs/cut-page-spec.md](docs/cut-page-spec.md), and intentional differences are recorded in [docs/deviations.md](docs/deviations.md).

## License

vidcut's source code is released under the [MIT License](LICENSE).

vidcut links the FFmpeg libraries and loads libmpv at runtime. Both are LGPL or GPL depending on how they were built, and Ubuntu's builds are GPL. Binaries you distribute together with those libraries must therefore comply with their licenses. vidcut's own source stays MIT.
