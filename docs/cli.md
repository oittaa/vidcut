# Driving vidcut from scripts and agents

**MCP** ([docs/decisions/agents.md](decisions/agents.md), deviation X10): `vidcut mcp [-p FILE]` serves the Model Context Protocol on stdin/stdout. Every command is a tool (`edit.split` is `edit_split`, with its JSON Schema), plus `use_project` to pick or create the project. Calls run like the CLI's: inside the GUI that has the project open, or headless. `frame_render`, `gui_screenshot` and `storyboard_render` return images. To add it to Claude Code: `claude mcp add vidcut -- vidcut mcp -p /abs/talk.vidcut`. Other clients take `{"command": "vidcut", "args": ["mcp", "-p", "/abs/talk.vidcut"]}`.

**Folders:** `~/.config/vidcut`, `~/.cache/vidcut` and `~/.local/state/vidcut` below are where vidcut keeps files on Linux. README.md's table says where they are on Windows and macOS.

**Commands are the protocol's methods** (`crates/vidcut/src/cli.rs` generates the CLI from the request schema):
- Method `group.verb` is the command `vidcut group verb`: dots become spaces, underscores hyphens (`edit.place_on_top` is `vidcut edit place-on-top`; MCP calls it `edit_place_on_top`).
- Every parameter is the flag of its name: `volume_db` is `--volume-db`. A true/false parameter that defaults to false is a bare flag (`--lift`). One that may be left unset takes a value (`--locked true`). Lists take commas or repeated flags (`--at 1m,1m30s`). Objects take JSON inline, as `@FILE` or `-` for stdin.
- A command's main object (ids, files, the time to seek to) may go without its flag, in order: `vidcut media relink m1 new.mp4`.
- `vidcut schema` prints the whole protocol as JSON Schema, and `vidcut schema edit.trim` prints one method's params. `vidcut --help` and `vidcut <group> <verb> --help` show the parameters and examples.
- Not requests: `vidcut` alone starts the GUI, `vidcut instances`, `vidcut mcp`, `vidcut rpc`.

**Basic use:**

```bash
vidcut project create talk.vidcut           # format from the first video added, or --fps 25 --width 1920 --height 1080; --start-tc 01:00:00:00
export VIDCUT_PROJECT=talk.vidcut           # or pass -p FILE every time
vidcut media add cam.mp4 screen.mkv --json  # -> media ids m1, m2
vidcut edit append m1 --range 10s..2m
vidcut edit split --at 1m,1m30s
vidcut edit join --at 1m30s                 # joins the parts again (spec D49)
vidcut edit delete V1@1m10s                 # the clip under 1m10s; ripples Track 1
vidcut timeline get --json                  # clips and gaps with frames and timecodes
vidcut history undo                         # works across separate runs
```

**Times:**
- In strings, every time needs a unit: `450f`, `15s`, `1m30s`, `250ms`, `00:01:00:00` (timecode; `00:01:00;00` drop-frame, decision D47), `HH:MM:SS.mmm`, `start`, `end`, `in`, `out`.
- In JSON params, a bare integer is a frame number.
- Ranges are half-open: `A..B` excludes B, and `A..+D` is D long.

**Clips:** refer to them by id (`c17`) or by position (`V1@<time>`, `V2@1m`). `vidcut edit slide c3 --by 5f` moves a clip keeping its media, and its neighbors give and take. `vidcut edit change-speed c3 --speed 0.5` plays a clip at half speed (`--speed=-1` backward, `0` a freeze frame at the playhead or `--at`), and `--ripple false` keeps its length instead of its source (spec D48).

**Tracks:**
- `vidcut track list` lists them. `track add video|audio [--count N]`, `track remove V3 [--force]` (`--force` also deletes its clips), and `track set A1 --volume-db -6 --name Music` with `--locked`, `--muted`, `--solo` and `--video-enabled`, each `true` or `false`.
- Track ids are positions: V1, V2, … from the bottom and A1, A2, … (spec D27). Removing a track renumbers the later ones, so list them again afterwards.
- Edit functions take `--track V2` (the target track) and `--audio-only`/`--video-only`. Naming the next new track (V3 when V2 is the last) creates it. Media without video go to A1 by default.
- `vidcut edit place-on-top m4 --range 5s..+3s` puts B-roll on the first track above everything at the playhead (or `--at`, or the timeline marks). The result names the clip and the track. `vidcut edit move c7 --to 2m --track V2` moves clips between tracks.
- Track 1 ripples. Clips on other tracks follow the Track 1 picture under their first frame (spec D21). Other tracks ripple only where Track 1 is empty (D22). Only Track 1 pushes clips: inserts elsewhere overwrite (D23).
- Export, `frame render` and the preview show the highest visible clip and mix every audible track with its fader. Mute and solo apply to the export too.

**Stills and image sequences (spec D38, D39):**
- `vidcut media add photo.jpg` adds a still. Edited without `--range` it lasts the project's still duration (5 s; `vidcut project set --still-duration 3s`). A range or trim makes it up to 17 h 40 min long.
- `vidcut media add renders/frame_000001.png --sequence [--fps 60]` adds the numbered files in a row around it as one clip (`frame_[000001-010560].png`), at the project's rate unless `--fps` is given. Without `--sequence` a numbered image is a still. Exporting a sequence fails if a frame's file is missing.
- `vidcut media get` reports `still: true`, or `sequence: {pattern, first, count}`.

**Proxies (spec D41, deviation X18):**
- `vidcut proxy generate [m1 m2…] [--width 960]` writes a proxy of each video or image sequence named, or of every one in the pool, in the codec the preferences' `proxies.codec` names: `auto` (the default), `dnxhr`, `av1`, `hevc`, `prores` or `h264` (spec D41). `auto` takes ProRes Proxy where the GPU makes and plays it (a Mac's, NVIDIA's), else HEVC or AV1 on the GPU, else DNxHR LB. Proxies of a 10-bit source keep its 10 bits where the encoder takes them: AV1, HEVC and ProRes. A 10-bit proxy keeps an HDR source's PQ or HLG and a PQ source's static HDR metadata, and an 8-bit one (H.264, DNxHR LB) is SDR, HDR tone-mapped. `vidcut probe` on a proxy's file says its `hdr`. `vidcut preferences get` says what each codec choice uses on this machine, and the result's `made_with` says how each proxy was made (`decoded on the GPU (NVDEC), frames stayed there, AV1, NVIDIA hardware (NVENC)`). Its `copies` counts the frames copied down from the GPU and up to it, with their bytes. Naming a still or an audio-only file is an error. It shows progress like an export, and Ctrl-C stops it without leaving a file. In a running GUI the encode is a job (`kind: "proxy"`), which `vidcut jobs list` lists.
- `--auto` takes only what the automatic rules pick (`proxies.*`). A running GUI makes those itself while `use_proxies` is on, as jobs with `origin: "session"`. Headless runs never do.
- `vidcut proxy remove [m1…]` deletes the proxy files and clears them. `vidcut project set --use-proxies true` plays fresh proxies in the preview. `vidcut media get` reports the proxy path, and `stale: true` when the source size or modification time no longer matches.

**Missing media (deviation X1):** `vidcut media list` marks media whose files are missing (`missing: true` in JSON).
- `vidcut media relink --search /mnt/footage` looks for all of them by file name there and in its subfolders, and relinks each file that is the same one (size, streams, fingerprint). `media relink m1,m4 --search DIR` looks for those only, and `--dry-run` says what it would do.
- The result lists `relinked`, `different` (found by name, but another file, with `why`) and `not_found`. `vidcut media relink m1 new.mp4` uses that file either way, saying how it differs if it does.

**Clip Color (spec §3):** `vidcut media set m1 --color teal` colors a pool item, and its clips show it. `vidcut clip set c4 --color pink` gives a clip its own. `--color ""` clears either.

**Bins:** `vidcut bins list` lists them. `bins add "Interviews/Day 1"` (parents too), `bins rename A B`, `bins remove A` (its media move up a level, deviation X6), `media add FILE --bin A`, `media move m1 m2 --bin A` (`""` is the top level). `media list` shows each item's bin.

**JSON output:**
- `--json` prints `{"rev", "changes": {"created", "modified", "deleted"}, "result", "via"}`. When the undo history could not be saved, `warnings` says so, and the edit is in the file.
- Errors print `{"error": {"code", "message"}}` and exit with the matching code:

| Code | Error |
|---|---|
| 0 | ok |
| 1 | `INTERNAL` |
| 2 | `USAGE` |
| 3 | `NOT_FOUND` |
| 4 | `INVALID_EDIT` |
| 5 | `CONFLICT` |
| 6 | `NO_INSTANCE` |
| 7 | `MEDIA` |
| 8 | `EXPORT` |
| 9 | `CANCELLED` |
| 10 | `DEPENDENCY` |

**FFmpeg's parts:** `vidcut doctor` lists what this FFmpeg build lacks of the parts vidcut uses (`ffmpeg_missing`: each one's name, kind, library and use). An export that needs one fails with `DEPENDENCY` before writing anything.

**Safe automation:**
- `--dry-run` reports `changes` without writing anything.
- `--if-rev N` fails with `CONFLICT` if someone else changed the project after you read revision `N`. Every request on the project checks it: edits, exports, frames, measurements, queries, and live ones such as seeks and screenshots. An export encodes the project as it was when it started, whatever edits follow. The project's `edited` (milliseconds since 1970 UTC) is for people, and the GUI's status bar shows it. Compare `rev`, not times.
- `vidcut apply edits.jsonl` (or `-` for stdin) applies many requests atomically, as one undo step. The format is one `{"method", "params"}` object per line.
- Parallel `vidcut` processes on one project are serialized by a lock and never corrupt it.

**Raw requests:** `vidcut rpc edit.split '{"at": ["1m", 2700]}'` sends one exactly as written.

**Export and stills:**
- `vidcut export start out.mp4` renders the timeline: its In–Out range when both marks are set, else everything.
  - The extension picks the container, and also the preset unless `--preset` is given.
  - Extra options: `--preset youtube-1080p`, `--encoder nvenc|vaapi|amf|qsv|videotoolbox|cpu|auto`, `--set video.quality=20`, `--range 1m..2m`, `--per-clip`, `--audio-only`, `--overwrite`.
  - With NVENC, VAAPI on the render node the pieces decode on, or AMF on their Direct3D 11 adapter (and `gpu.acceleration` on), frames are made on the GPU where the piece qualifies (export.md §5). The result's `gpu_frames` counts them. With `gpu.decoding` on too, pieces decode on the GPU the viewer decodes on, whatever the encoder, and `gpu_decoded` counts those frames. The result's `copies` counts the frames copied down from the GPU and up to it, with their bytes.
  - `vidcut export plan out.mp4` (or `export start` with `--dry-run`) prints the plan and the chosen encoder without encoding.
  - Dynamic range and bit depth (spec D44): `video.hdr` is `auto`, `sdr`, `pq` or `hlg`, and `video.bit_depth` is chosen unless given. `--set video.codec=hevc --set video.hdr=pq` exports HDR whatever the footage, and `video.hdr=sdr` tone-maps HDR sources. Each output's `color` in the plan and the result says what was chosen and why (`hdr`, `bit_depth`, `picture`, `why`). When HDR picture will be tone-mapped and SDR wasn't asked for, `notes` has `{"code": "hdr_tone_mapped", "message", "set"}`, where `set` holds the `--set` overrides that keep it HDR. `vidcut media get` reports each clip's `hdr`, `rotation`, interlaced `fields` and HDR metadata (`mastering`, `light`, `hdr10plus`, `dolby_vision`).
  - HDR metadata (spec D46): `video.hdr_metadata` is `auto`, `static`, `hdr10plus`, `dolby_vision`, `all` or `none`. `--set video.mastering_display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1)` and `--set video.max_cll=1000,400` give the values (x265's forms), and `video.max_cll=measure` measures every picture. Progress events and jobs carry `measuring: true` while it measures. Each output's `metadata` in the plan and the result says what is written and why (`mode`, `mastering`, `light`, `why`).
  - Passes (spec D45): `video.passes` is `1` or `2`. `--set video.bitrate=8000000 --set video.quality=null` targets a bitrate. The other settings are `video.first_pass=fast|full`, NVENC's `video.multipass=quarter|full` and `video.lookahead=N` (0–32). The plan's `video_encoder.passes` says how (`{"kind": "two_runs", "fast_first": true}`, `two_inside`, `one`), and the result's `passes` how many. Progress events and jobs carry `pass` and `passes`.
  - Ctrl-C cancels cleanly (exit 9, no partial file).
  - With a GUI running, `--background` returns a job id at once. Follow it with `vidcut jobs` commands (below).
- `vidcut presets list` lists the presets (`--json` includes their settings, and `unavailable`: why this machine can't export one). `vidcut presets get NAME` shows one.
  - `vidcut presets save web --from youtube-1080p --set video.quality=22 [--quick-export]` saves a user preset to `~/.config/vidcut/presets/web.toml`. `--settings JSON|@FILE` gives full settings instead. `vidcut presets delete web` removes it.
- `vidcut encoders list` shows which GPU encoders work here, on each GPU (cached, `--refresh` tests again), whether or not `gpu.acceleration` lets vidcut use them, and why each codec and encoder can't encode here with the GPU in use (`video`, `audio`; `null`: it can).
- `vidcut decoders list` shows which codecs each GPU decoder here decodes: a short clip of each (H.264, 10-bit HEVC, VP9, 10-bit AV1, ProRes), made once in the cache folder and decoded through on each. A codec vidcut leaves to the CPU on that GPU says why. On NVIDIA's GPU, ProRes says `Vulkan`: NVDEC doesn't decode it, FFmpeg's Vulkan decoder does.
- `vidcut preferences get` shows the application preferences (`~/.config/vidcut/preferences.toml`) and what the thumbnail cache holds. `vidcut preferences set thumbnails.cache_limit_mb=256` changes them (`preferences.set` takes `{"thumbnails": {"disk_cache": false}}`; turning the cache off empties it). `thumbnails.timeline_memory_mb` (256) is the memory a running GUI keeps timeline tiles in. `player.memory_mb` and `player.video_memory_mb` are the viewer's for decoded pictures (`0`, the default, is automatic, and `preferences get` says what `player_memory_auto_mb` comes to here). A running GUI takes a change at once.
  - `appearance.theme=neon` (File > Preferences: Appearance) picks the GUI's theme (deviation X22): `native` (the default, light or dark as the computer is set, in its accent color), `starship`, `neon`, `cybertruck`, `midnight`, `studio`, `native-dark`, `native-light`, `classic`, or one of yours. `preferences get` lists them (`themes`) and the folder yours go in (`themes_folder`). A path to a `.toml` file copies it into that folder and picks it. A running GUI shows a change at once, also an edit of the theme's file.
  - `gpu.acceleration=false` (File > Preferences: GPU) turns off all GPU work: exports and proxies take software encoders, and the preview decodes in software at once. `gpu.decoding=false` decodes everything on the CPU, and the GPU only encodes. Both are on by default. `player state`'s `hwdec` says what decodes: `nvdec`, `vaapi`, `d3d11va`, `videotoolbox`, `vulkan` (ProRes on NVIDIA's GPU) or `no`.
  - `gpu.device=<id>` moves all GPU work to that GPU. `preferences get` lists the GPUs (`gpus`). An id is the GPU's PCI vendor and device ids and its slot (`10de:2702@01:00.0`), the same on Linux and Windows on one computer. A card moved to another slot is still found while it is the only one of its model. `gpu_chosen` is the GPU the id finds, null when none. Empty (the default), or while the chosen GPU isn't there, it is NVIDIA's when there is one, else the first other (`gpu_automatic`). An export or preset naming an encoder the GPU in use doesn't have encodes with the GPU's own, or on the CPU, and a note says so (`encoder_changed`).
  - `player.audio_device=<id>` (File > Preferences: Player, Sound) plays the preview's sound on that device, an id `vidcut doctor` lists under `audio_outputs`. Empty is the system's default. A device that isn't connected plays on the default: `player state`'s `audio_device` says which device plays (null: the system's default), and `audio_error` why it isn't the one chosen.
- `vidcut frame render --at 1m --out f.png [--width 640]` renders what the timeline shows at a frame. `--media m1` renders a media frame instead. Use it to look at cuts.

**Automatic tools (spec D29–D32):**
- `vidcut edit ripple-delete-silence c1 [c2…] [--min 0.5s --threshold-db -40 --pre-head 0.15s --post-tail 0.15s]` (the defaults shown) cuts silent parts out of clips and closes the gaps, as one undo step. `--dry-run` lists the parts.
- `vidcut edit detect-scene-cuts c1` splits a clip where its shots change (`--threshold`, scene score 0–100, default 10).
- `vidcut analyze silence|scenes|black [--media m1 | --clips c1,c2]` and `vidcut analyze loudness [--range a..b]` measure without changing anything. Media positions are source frames, and clip positions are timeline frames.
- `vidcut timeline boring [--boring 30s --jump 10f]` lists clips too long and too short (the Boring Detector).
- `vidcut storyboard render --out board.png [--every 5s --columns 4 --width 320]` draws a picture of every shot with timecode and clip id: the quickest way to look at a whole edit.
- `vidcut export start out.mp4 --set audio.loudness=-14` normalizes the mix to −14 LUFS (one gain, true peak ≤ −1 dBTP). The result reports the gain.
- Measuring decodes a file once. The results are cached (`~/.cache/vidcut/analysis/`), so trying other settings is quick.

**Timeline interchange (OpenTimelineIO, deviation X9).** What the file contains is that deviation.
- `vidcut export start cut.otio` writes the whole timeline, or `--range`. `cut.otioz` bundles the media (a zip; in a GUI it runs as a job), `cut.otiod` the same as a folder. Tracks carry their names (`track set A1 --name Music`).
- `vidcut timeline import cut.otio` (or `.otioz`, or an `.otiod` folder) adds the file's media to the pool and the file's tracks after the last clip: video tracks onto V1, V2, … with the audio that belongs to their clips, other audio tracks onto A1, A2, …. An empty timeline takes the file's frame rate, start timecode and track settings.
  - `--mode replace` replaces the timeline instead, `--mode compound` adds one compound clip that holds the file's tracks, and `--mode media` adds only the media.
  - A file vidcut wrote brings its media as its project has them, with their colors and marks, into a bin named after the file. A file the pool has keeps its pool item.
  - `--flatten` decomposes the compound clips it holds into their clips, and `vidcut edit decompose --clips c5` decomposes one later (deviation X9).
  - An `.otioz`'s media unpack next to it, into a folder named after it (`cut/media/…`), or into `--unpack DIR`. A dry run unpacks nothing and says what it would.
  - Media that moved: paths relative to the file and a project folder moved with the file are found by themselves. Otherwise `--search /mnt/footage` looks for them by file name, subfolders too. Missing media fail the import (exit 7) with the list of files.
  - What can't be imported (clips overlapping others on their track) is listed in `warnings`. Clip effects, transitions, speed changes, generators, titles and compound clips from another editor are kept and written back (deviation X9). `timeline get` lists them in `effects`, each track's `transitions`, `speed`, `generator` and `compound`.
- `vidcut project create p.vidcut --from cut.otio` makes a project of the file. A file vidcut wrote of its whole timeline gives the project it holds. Another editor's timeline is imported, in the format given or else the file's.
- Raw: `timeline.import` also takes the document inline as `otio` (relative media paths then can't be found but by `search`).

**Live control of a running GUI** ([docs/decisions/gui.md](decisions/gui.md)):
- **Routing:** when a GUI has the project open (`vidcut -p FILE` starts one), every project command runs inside it. You and the human then edit one live session: the GUI updates at once, and `via` in the JSON says `gui:<pid>`.
  - `--headless` forces a separate run, and `--instance PID|SOCKET` (or `VIDCUT_SOCKET`) picks a GUI.
  - `vidcut instances` lists running GUIs.
- **Live-only commands:**

| Command | What |
|---|---|
| `vidcut player state` | viewer mode, both playheads and whether each one's picture is decoded (`ready`; the one not shown is kept ready, so switching shows it at once), the source clip, selection, what decodes (`hwdec`); at the top, the audio device (`audio_device`, `audio_error`), the viewer's late presents and skipped frames while playing (`late`, `skipped`); in `stats.playback`, the frames copied down from GPU memory and up to it since the player started, with their bytes (`copied_down`, `copied_up`), which a switch of decoder doesn't reset |
| `vidcut player seek 10s [--viewer timeline\|source_clip\|source_tape]` | returns once the frame is on screen (`CONFLICT` if someone moved the viewer first); `+10f`/`-2s` are relative |
| `vidcut player step [-]N`, `player play [--speed -2]`, `player pause`, `player toggle` | negative speeds play backwards, the sound reversed; other speeds keep its pitch |
| `vidcut viewer set source_clip\|source_tape\|timeline`, `viewer load m1 [--at 5s]` | what the viewer shows |
| `vidcut selection set --clips c1,c2 --media m3`, `selection get` | the selection |
| `vidcut gui screenshot --out v.png [--target window]` | the viewer picture as shown (works while minimized), or the whole window |
| `vidcut gui log [--since N]` | what everyone (the human too) did: each entry's `summary` in words, and the request as sent |
| `vidcut gui window --size 1600,1000`, `project open FILE [--create [--fps 25 --width 1920 --height 1080] [--force]]`, `gui quit` | window and project. `--create` opens a file already there, and `--force` replaces it with the new project. `project open cut.otio` opens a project file made next to it, `cut.vidcut` (or `--save-as FILE`), as `project create --from` makes it |
| `vidcut jobs list`, `jobs get\|wait\|cancel J [--timeout S]` | export and proxy jobs, the GUI's own included. They run one at a time, and the others are `queued`. `wait` fails like the job did |
| `vidcut pool set --bin A --sort name --descending true --view list` | the bin the Media Pool shows and its order, which Source Tape follows, and its view: `thumbnail` (Grid), `filmstrip` or `list` |
| `vidcut player mute [true\|false]` | the viewer's monitoring mute (not the export); without a value, toggles |
| `vidcut player audio-scrubbing [true\|false]` | audio scrubbing (decision D42): steps and paused seeks play their frame's sound; without a value, toggles (Shift+S) |
| `vidcut player stats [true\|false]` | stats for nerds (deviation X20): a panel over the viewer, also in its right-click menu; without a value, toggles. Prints the facts, which `player state` has as `stats` |
| `vidcut mixer preview A1 -12` | a track's level heard now, without changing the project (`track set --volume-db` keeps one); `live: false` while the viewer shows the source |
| `vidcut gui show quick_export\|export\|jobs\|log\|mixer\|preferences` | opens that window for the person at the GUI |

- **Looking at the result:** `vidcut player seek 1m && vidcut gui screenshot --out shot.png` shows exactly what `vidcut frame render --at 1m --out f.png` renders. Both are frame-exact.
- **Exporting alongside the human:** `vidcut export start out.mp4 --background` returns at once. The export shows in the GUI's Jobs window, and `vidcut jobs wait j1` blocks until it's done. An export the human starts from Quick Export is a job too, so `vidcut jobs list` sees it.
- **Source marks and clipboard work headless too:**
  - `vidcut media marks set m1 --in 5s --out 10s`: edits without `--range` then use these marks.
  - Source Tape marks can span clips (spec D17): `vidcut tape marks set --in 200f --out 300f --order m1,m2` (tape frames of those media end to end), then edits with `--tape` instead of a media id, such as `vidcut edit append --tape`, place the span as one block.
  - `vidcut edit copy c3 --json`, then `vidcut edit paste --clips @clips.json --at 1m`.
- **Paths:** the CLI makes paths absolute before routing, those in `apply` batches too, since a GUI may run in another directory. In raw `rpc` requests, use absolute paths.
