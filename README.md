# vidcut

A simple, fully scriptable video cutting tool for Linux, modeled on DaVinci Resolve's **Cut page**.

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
- **Export:**
  - Quick Export presets (H.265 Master, AV1, ProRes 422 HQ, YouTube, TikTok), plus your own
  - an Export dialog with codec settings, GPU encoding (NVENC, VAAPI), In–Out range, per-clip export and audio-only export
  - exports run in the background, with progress, ETA and cancel
  - HDR exports keep their sources' mastering display and light levels, HDR10+ and Dolby Vision
- **Automatic tools:** Ripple Delete Silence cuts the pauses out of a talk; Detect Scene Cuts splits an edited video back into its shots; the Boring Detector flags clips that run too long or flash by; exports can be normalized to a loudness target (−14 LUFS for YouTube).
- **Interchange:** import and export timelines as OpenTimelineIO, which DaVinci Resolve and other editors read and write: `.otio` files, and `.otioz` bundles that carry the media along. Files are written the way Resolve writes them, so an edit goes to Resolve and back.
- **Everything scriptable.** Every action in the GUI is also a CLI command and a JSON-RPC call on the running app, so scripts and AI agents can edit with you. `vidcut mcp` offers it all to AI agents over the Model Context Protocol.
- **Deliberately not included:** effects, transitions, titles or color grading.

## Requirements

Ubuntu 26.04 (or similar) with FFmpeg:

```bash
sudo apt install libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libswscale-dev libswresample-dev libavdevice-dev libasound2-dev libclang-dev rustup
```

The GUI draws with Vulkan (the driver any desktop has: Mesa's for AMD and Intel, NVIDIA's own) and plays sound through ALSA, which PipeWire and PulseAudio serve. Nothing else is needed at run time: vidcut plays video with its own player, on the FFmpeg libraries. On NVIDIA GPUs the player decodes H.264, HEVC, AV1 and VP9 on the GPU.

vidcut works with FFmpeg 8.0 (Ubuntu 26.04 LTS) up to the newest release, 9.0; both ends are tested.

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

Everything you, scripts and agents do is listed in the **Command log** (top right) in words, such as "Split at 00:01:05:12" or "Undo: Delete c14". Hover an entry for the request it sent; click it to copy that as a shell command.

**Exporting.** **Quick Export** (top bar) exports the In–Out range, or the whole timeline when no marks are set, with one of the presets shown as tiles. **Export** opens every setting: start from a preset, change the codec, quality, size, range or output, and see the plan (the encoder that will run, the files and their lengths) and the same export as a shell command before you start. Save the settings as your own preset, optionally added to Quick Export. Exports run in the background; **Jobs** (top right) shows their progress and lets you cancel them. Phone video shows upright, interlaced video is deinterlaced, and the output follows the footage: mostly-HDR timelines stay HDR in HEVC, AV1 and VP9, and 10-bit footage stays 10-bit where players handle it. At a target bitrate, exports make two passes where the encoder can; x264, x265 and VP9 then land within about 1% of it. When HDR footage will be tone-mapped (H.264, or HDR in the minority), the Export dialog says so in one quiet line with a button to keep it HDR; choose SDR or HDR yourself in Export to decide. **File > Import/Export Timeline** reads and writes OpenTimelineIO files.

**Preferences.** **File > Preferences** (Ctrl+,) holds settings for vidcut as a whole, kept in `~/.config/vidcut/preferences.toml`. The pool's thumbnails are kept on disk (`~/.cache/vidcut/thumbs`, 512 MB by default), so a project opens with its thumbnails shown instead of decoding every clip again; you can change the limit or turn the cache off, which empties it. With **Proxies** ticked in the Media Pool, vidcut makes playback proxies by itself for heavy media: image sequences, video larger than 4K, and files above 150 Mb/s. Preferences sets those rules, the proxy size and the codec, or turns this off. Proxies are AV1 when the machine has a hardware AV1 encoder, H.264 otherwise; each choice in Preferences says what it uses. `vidcut preferences get` and `vidcut preferences set` show and change the same settings.

### Keys

**Playing and moving around**

| Key | Does |
|---|---|
| Space | Play or stop |
| L | Play forward (press again to go faster) |
| J | Play backward (press again to go faster) |
| K | Stop |
| K held, then J or L | Step one frame back or forward |
| ← | Back one frame |
| → | Forward one frame |
| Shift+← | Back one second |
| Shift+→ | Forward one second |
| ↑ | Previous edit |
| ↓ | Next edit |
| Shift+↑ | Previous marker |
| Shift+↓ | Next marker |
| Home | Start of the timeline |
| End | End of the timeline |
| = | Go to a timecode you type |
| + | Go forward by the time you type |
| − | Go back by the time you type |
| Q | Switch the viewer between Timeline and the last source |
| Shift+S | Audio scrubbing on or off: steps and drags of a paused playhead play the sound under them |

Typed timecodes fill from the right: 100 is 1 second, 10000 is 1 minute.

**Marks**

| Key | Does |
|---|---|
| I | Mark In (in the viewer, or on the pool clip under the pointer) |
| O | Mark Out |
| Shift+I | Go to the In mark |
| Shift+O | Go to the Out mark |
| Alt+X | Clear In and Out |

**Edit buttons**

| Key | Does |
|---|---|
| F9 | Smart Insert |
| Shift+F12 | Append |
| Shift+F10 | Ripple Overwrite |
| F10 | Overwrite |
| F12 | Place On Top |

**Timeline**

| Key | Does |
|---|---|
| Ctrl+\\ | Split at the playhead |
| Ctrl+B | Split at the playhead (no AltGr needed on any keyboard layout) |
| Delete | Delete the selected clips and close the gap (with In and Out set: delete that range) |
| Alt+Delete | Delete the selected clips and leave the gap (lift) |
| Ctrl+C | Copy |
| Ctrl+X | Cut |
| Ctrl+V | Paste at the playhead |
| Ctrl+Z | Undo |
| Ctrl+Shift+Z | Redo |
| Ctrl+A | Select all clips |
| Ctrl+Shift+A | Select nothing |
| Alt+Shift+V | Select the clips under the playhead |
| D | Disable or enable the selected clips |
| Ctrl+D | Change the selected clip's duration |
| M | Add a marker (on a marker: edit it) |
| Alt+M | Remove the marker at the playhead |
| V | Select the edit nearest the playhead |
| U | Switch the selected edit between both sides, its Out and its In |
| , | Trim the selected edit one frame left |
| . | Trim the selected edit one frame right |
| Shift+, | Trim the selected edit one second left |
| Shift+. | Trim the selected edit one second right |
| N | Snapping on or off (during a drag: for that drag only) |
| W | Dynamic Trim: J, K and L trim the selected edit instead of playing, and K applies the trim |
| Ctrl+mouse wheel | Zoom the lower timeline |

**Other**

| Key | Does |
|---|---|
| Ctrl+I | Import media (into the open bin) |
| Ctrl+Shift+N | New bin |
| Ctrl+, | Preferences |

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
vidcut -p talk.vidcut player seek 1m              # moves the GUI's playhead
vidcut -p talk.vidcut gui screenshot --out shot.png   # what the viewer shows, frame-exact
vidcut -p talk.vidcut gui log                     # what was done, by whom
vidcut -p talk.vidcut export start talk.mp4 --background   # shows in the GUI's Jobs window
vidcut -p talk.vidcut jobs wait j1                # blocks until that export is done
```

## Try the CLI

```bash
vidcut project create talk.vidcut
vidcut -p talk.vidcut media add cam.mp4 screen.mkv
vidcut -p talk.vidcut edit append m1 --range 10s..2m
vidcut -p talk.vidcut edit split --at 1m
vidcut -p talk.vidcut edit delete V1@1m10s       # ripple delete the clip under 1m10s
vidcut -p talk.vidcut edit place-on-top m2 --range 5s..+3s --at 30s   # B-roll over Track 1
vidcut -p talk.vidcut media add music.flac       # m3
vidcut -p talk.vidcut edit overwrite m3 --at start   # sound alone lands on audio track A1
vidcut -p talk.vidcut track set A1 --volume-db -12 --name Music
vidcut -p talk.vidcut timeline get
vidcut -p talk.vidcut history undo
vidcut -p talk.vidcut export start talk.mp4 --preset youtube-1080p   # GPU-encoded when available
vidcut -p talk.vidcut export start talk.flac                          # audio only
vidcut -p talk.vidcut frame render --at 1m --out check.png            # look at a frame
vidcut -p talk.vidcut export start cut.otio                           # the edit, for another editor
vidcut -p talk.vidcut export start cut.otioz                          # the edit and its media, in one file
vidcut project create cut2.vidcut && vidcut -p cut2.vidcut timeline import cut.otio  # and back in
vidcut presets save web --from youtube-1080p --set video.quality=22 --quick-export
vidcut -p talk.vidcut edit ripple-delete-silence c1   # cut the pauses (0.5 s and longer)
vidcut -p talk.vidcut storyboard render --out board.png             # every shot on one page
vidcut -p talk.vidcut export start talk.mp4 --set audio.loudness=-14  # YouTube loudness
```

Every command is a method of the protocol: `vidcut edit split` is `edit.split`, and its flags are the method's parameters (`--volume-db` for `volume_db`). Every command takes `--json` for machine-readable output. `vidcut schema` lists the full protocol. Add `--headless` to work on the file directly even while a GUI has it open.

## Moving a project

- **Keep the media in the project's folder**, or under it: the project file stores their paths relative to it, so the folder can move or go to another computer and the project still finds them. So do proxies made beside the project. Media elsewhere keep their absolute paths.
- **Media that moved** are marked MISSING in the Media Pool. Relink Media in its header asks for a folder, looks there and in its subfolders for every missing file by name, and relinks the ones that are the same file (size, streams and a fingerprint of the content); the others are listed with why. From the CLI: `vidcut -p talk.vidcut media relink --search /mnt/footage`.
- **To DaVinci Resolve:** export `cut.otioz` (File > Export Timeline with Media (OTIOZ)…), which carries the media, and import it in Resolve with File > Import Timeline: the media unpack next to it and link themselves. A `.otio` names the media by absolute path; when they aren't there, Resolve asks for the folder they are in and looks in its subfolders too. Clips still offline relink from the Media Pool with Relink Media or Relink Selected Clips.
- **Back from Resolve or another computer:** `vidcut -p cut.vidcut timeline import cut.otio` finds media named relative to the file, or in a project folder that moved with it; `--search /mnt/footage` looks for the rest by file name, subfolders too. In the GUI, File > Import Timeline (OTIO)… asks for that folder.

## Use with AI agents

`vidcut mcp` serves the Model Context Protocol, so an AI agent can use every vidcut command as a tool. If the project is open in the GUI, you watch the agent's edits happen. With Claude Code:

```bash
claude mcp add vidcut -- vidcut mcp -p /path/to/talk.vidcut
```

Agents can look at their work: frames, screenshots and storyboards come back as images. [docs/cli.md](docs/cli.md) has the details.

## What's next

The use cases vidcut is for, and what comes next, are in [docs/roadmap.md](docs/roadmap.md). Open items are in [docs/todo/](docs/todo/), one file each.

## Contributing

Read [AGENTS.md](AGENTS.md) first; it's written for AI agents but applies to humans too. Behavior follows [docs/cut-page-spec.md](docs/cut-page-spec.md), and intentional differences are recorded in [docs/deviations.md](docs/deviations.md).

## License

vidcut's source code is released under the [MIT License](LICENSE).

vidcut links the FFmpeg libraries, which are LGPL or GPL depending on how they were built; Ubuntu's builds are GPL. Binaries you distribute together with them must therefore comply with their licenses. vidcut's own source stays MIT.
