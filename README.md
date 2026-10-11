# vidcut

A simple, fully scriptable video cutting tool for Linux, modeled on DaVinci Resolve's **Cut page**.

> **Status: early development.**
> - Done:
>   - **M0:** spikes proved frame-exact decoding and mpv preview.
>   - **M1:** the project model, Cut-page edit operations and the headless CLI.
>   - **M2:** frame-exact export with presets, GPU encoding, ranges, per-clip and audio-only output.
>   - **M3:** the GUI shell. It has the media pool, the viewer (Source Clip, Source Tape, Timeline), transport and marks, the edit buttons and an overview timeline. It also has live control: CLI commands and agents drive the running GUI.
>   - **M4:** timeline editing with the mouse. The dual timeline has filmstrips and waveforms. You can drag and drop from the pool, move, trim, roll, slip and make J/L cuts, with snapping, Safe Edit, markers and the Trim Editor.
>   - **M5:** exporting from the GUI: Quick Export, an Export dialog with every setting, and a Jobs window with progress and cancel. Timelines move to and from other editors as OpenTimelineIO.
>   - **M6:** several tracks: B-roll above Track 1 that follows its edits, music on audio tracks, Place On Top, a mixer with meters, mute and solo, and bins in the media pool.
>   - **M7:** for agents and for speed: an MCP server, Ripple Delete Silence, scene and black detection, the Boring Detector, Dynamic Trim, storyboards and loudness-normalized export.

## What it will do

- **Cut-page editing:**
  - a media pool with bins, and a source/timeline viewer (Source Clip, Source Tape, Timeline)
  - a dual timeline: an overview of the whole program plus a detail view
  - Smart Insert, Append, Ripple Overwrite, Overwrite, Place On Top
  - split, ripple delete, trimming (resize, roll, slip, J/L cuts)
  - several tracks: B-roll that follows the main story on Track 1, music on audio tracks
  - a mixer with faders, mute, solo and meters
  - snapping, markers, undo
- **Export:**
  - Quick Export presets (H.264/H.265 Master, ProRes 422 HQ, YouTube, Vimeo, TikTok), plus your own
  - an Export dialog with codec settings, GPU encoding (NVENC, VAAPI), In–Out range, per-clip export and audio-only export
  - exports run in the background, with progress, ETA and cancel
- **Automatic tools:** Ripple Delete Silence cuts the pauses out of a talk; Detect Scene Cuts splits an edited video back into its shots; the Boring Detector flags clips that run too long or flash by; exports can be normalized to a loudness target (−14 LUFS for YouTube).
- **Interchange:** import and export timelines as OpenTimelineIO (`.otio`), which DaVinci Resolve and other editors read and write.
- **Everything scriptable.** Every action in the GUI is also a CLI command and a JSON-RPC call on the running app, so scripts and AI agents can edit with you. `vidcut mcp` offers it all to AI agents over the Model Context Protocol.
- **Deliberately not included:** effects, transitions, titles or color grading.

## Requirements

Ubuntu 26.04 (or similar) with FFmpeg 8 and libmpv:

```bash
sudo apt install libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libswscale-dev libswresample-dev libavdevice-dev libclang-dev libmpv2 rustup
```

Rust comes from [rustup](https://rustup.rs). The repository pins its Rust version in `rust-toolchain.toml`, and rustup downloads that toolchain the first time you build.

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

**Exporting.** **Quick Export** (top bar) exports the In–Out range, or the whole timeline when no marks are set, with one of the presets shown as tiles. **Export** opens every setting: start from a preset, change the codec, quality, size, range or output, and see the plan (the encoder that will run, the files and their lengths) and the same export as a shell command before you start. Save the settings as your own preset, optionally added to Quick Export. Exports run in the background; **Jobs** (top right) shows their progress and lets you cancel them. Phone video shows upright, interlaced video is deinterlaced, and HDR video is tone-mapped for ordinary (SDR) exports; choose an HDR range in Export (HEVC, AV1 or VP9) to keep it HDR. **File > Import/Export Timeline** reads and writes OpenTimelineIO files.

**Preferences.** **File > Preferences** (Ctrl+,) holds settings for vidcut as a whole, kept in `~/.config/vidcut/preferences.toml`. The pool's thumbnails are kept on disk (`~/.cache/vidcut/thumbs`, 512 MB by default), so a project opens with its thumbnails shown instead of decoding every clip again; you can change the limit or turn the cache off, which empties it. `vidcut preferences` shows and changes the same settings.

| Keys | |
|---|---|
| Space, J / K / L | play/stop, shuttle (repeat J/L for speed; hold K and tap J/L to step) |
| ← / →, Shift+← / → | one frame, one second |
| ↑ / ↓, Shift+↑ / ↓, Home / End | previous/next edit, previous/next marker, start/end |
| I / O, Alt+X, Shift+I / O | mark In/Out (source, timeline, or the pool clip under the pointer), clear, go to |
| Q | Viewer: Timeline ↔ last source mode |
| = / + / − | type a timecode (absolute or relative); digits fill from the right, so 100 is 1 second |
| F9, Shift+F12, Shift+F10, F10, F12 | Smart Insert, Append, Ripple Overwrite, Overwrite, Place On Top |
| Ctrl+\\, Delete, Alt+Delete | split at the playhead, ripple delete (or the In–Out range), lift |
| Ctrl+C / X / V, Ctrl+Z / Ctrl+Shift+Z | copy, cut, paste at the playhead; undo, redo |
| M, Alt+M, D | add marker (or edit the one at the playhead), remove marker, disable clip |
| V, U, `,` / `.` | select the nearest edit, cycle Out/In/both, trim it a frame (Shift: a second) |
| N, Ctrl+D | snapping on/off (only for the current drag if pressed mid-drag), change clip duration |
| Ctrl+I, Shift+Ctrl+N | import media (into the open bin), new bin |
| W | Dynamic Trim: J/K/L trim the selected edit instead of playing; K applies it |

**The timeline** follows the Cut page:
- The upper timeline shows the whole program; the lower one is a zoomed detail view (Ctrl+wheel) whose playhead stays in the middle.
- Where you grab a clip picks the operation:
  - its edge resizes it (Alt: without ripple);
  - the middle of an edit rolls it;
  - the handle at a clip's top center slips it;
  - the audio band at an edit makes a J/L cut;
  - the body moves it.
- Where you drop decides the edit: on an edit point inserts; on a clip replaces it, or overwrites if you hover first; past the end appends.
- Drag media from the pool or the viewer to edit it in. Double-click an edit for the Trim Editor.

**Tracks.** Track 1 holds the main story and closes up behind every edit. Clips on the tracks above it are B-roll: they stay with the Track 1 picture they sit over when Track 1 changes. Audio tracks (A1, A2, …) hold music and other sound. Click a track's name to make it the target of the edit buttons, and right-click it to add or remove tracks. Drop a clip on a row to put it on that track, or above the top row (below the bottom row) to make a new track. **Place On Top** (F12) puts the source on the first free track above everything at the playhead. The topmost visible clip is what you see. Every unmuted track is heard; **Mixer** (top bar) has a fader, mute and solo for each track, and mute and solo apply to exports too. The meter next to the viewer shows the level, and its speaker button mutes playback.

**Automatic tools.** Select clips and use **Actions** (above the timeline): *Ripple Delete Silence…* shows the pauses it would cut as red stripes before you press Remove, and *Detect Scene Cuts* splits clips at their shot changes. **Options > Boring Detector** marks clips that are too long (gray) or too short (red). **Dynamic Trim** (W) trims the selected edit with J/K/L while the viewer shows both sides of the cut.

**Bins.** Organize the pool into bins (File > New Bin, or right-click in the pool). Drag clips onto a bin, double-click a bin to open it, and use the path above the pool to go back up. Source Tape plays the open bin.

**Live control.** While the GUI has a project open, `vidcut` commands for that project run *inside* it, and the window updates at once. A script or an AI agent can therefore edit alongside you:

```bash
vidcut -p talk.vidcut edit split --at 1m          # shows up in the GUI immediately
vidcut -p talk.vidcut seek 1m                     # moves the GUI's playhead
vidcut -p talk.vidcut screenshot -o shot.png      # what the viewer shows, frame-exact
vidcut -p talk.vidcut log                         # what was done, by whom
vidcut -p talk.vidcut export talk.mp4 --background   # shows in the GUI's Jobs window
vidcut -p talk.vidcut jobs wait j1                # blocks until that export is done
```

## Try the CLI

```bash
vidcut new talk.vidcut
vidcut -p talk.vidcut media add cam.mp4 screen.mkv
vidcut -p talk.vidcut edit append m1 --range 10s..2m
vidcut -p talk.vidcut edit split --at 1m
vidcut -p talk.vidcut edit delete V1@1m10s       # ripple delete the clip under 1m10s
vidcut -p talk.vidcut edit place-on-top m2 --range 5s..+3s --at 30s   # B-roll over Track 1
vidcut -p talk.vidcut media add music.flac       # m3
vidcut -p talk.vidcut edit overwrite m3 --at start   # sound alone lands on audio track A1
vidcut -p talk.vidcut track set A1 --volume -12 --name Music
vidcut -p talk.vidcut timeline
vidcut -p talk.vidcut undo
vidcut -p talk.vidcut export talk.mp4 --preset youtube-1080p   # GPU-encoded when available
vidcut -p talk.vidcut export talk.flac                          # audio only
vidcut -p talk.vidcut frame --at 1m -o check.png                # look at a frame
vidcut -p talk.vidcut export cut.otio                           # the edit, for another editor
vidcut new cut2.vidcut && vidcut -p cut2.vidcut import cut.otio  # and back in
vidcut presets save web --from youtube-1080p --set video.quality=22 --quick-export
vidcut -p talk.vidcut edit ripple-delete-silence c1 --min 0.7s   # cut the pauses
vidcut -p talk.vidcut storyboard -o board.png                   # every shot on one page
vidcut -p talk.vidcut export talk.mp4 --set audio.loudness=-14  # YouTube loudness
```

Every command takes `--json` for machine-readable output. `vidcut schema` lists the full protocol. Add `--headless` to work on the file directly even while a GUI has it open.

## Use with AI agents

`vidcut mcp` serves the Model Context Protocol, so an AI agent can use every vidcut command as a tool. If the project is open in the GUI, you watch the agent's edits happen. With Claude Code:

```bash
claude mcp add vidcut -- vidcut mcp -p /path/to/talk.vidcut
```

Agents can look at their work: frames, screenshots and storyboards come back as images. AGENTS.md has the details.

## Milestones

| | |
|---|---|
| M0 | Scaffold; spikes for mpv preview and frame-exact decoding |
| M1 | Project model, Cut-page edit operations, headless CLI |
| M2 | Export engine (presets, GPU, range, per-clip, audio-only) |
| M3 | GUI shell, viewer, media pool, live control over a socket |
| M4 | Timeline editing (dual timeline, trimming, snapping, markers) |
| M5 | Export UI (Quick Export, Export dialog, jobs), OpenTimelineIO import/export |
| M6 | Multi-track (B-roll, music, Place On Top, mixer) |
| M7 | Agent extras (MCP server, Ripple Delete Silence, scene detection, Boring Detector, Dynamic Trim, storyboards, loudness) |
| M8 | Media coverage: rotation metadata, interlaced sources, HDR (tone mapping and HDR export) |

What comes next, and the use cases behind it, is in [docs/roadmap.md](docs/roadmap.md).

## Contributing

Read [AGENTS.md](AGENTS.md) first; it's written for AI agents but applies to humans too. Behavior follows [docs/cut-page-spec.md](docs/cut-page-spec.md), and intentional differences are recorded in [docs/deviations.md](docs/deviations.md).

## License

vidcut's source code is released under the [MIT License](LICENSE).

vidcut links the FFmpeg libraries and loads libmpv at runtime. Both are LGPL or GPL depending on how they were built, and Ubuntu's builds are GPL. Binaries you distribute together with those libraries must therefore comply with their licenses. vidcut's own source stays MIT.
