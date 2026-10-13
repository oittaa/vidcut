# Driving vidcut from scripts and agents

**MCP** ([docs/decisions/agents.md](decisions/agents.md), deviation X10): `vidcut mcp [-p FILE]` serves the Model Context Protocol on stdin/stdout. Every command is a tool (`edit.split` is `edit_split`, with its JSON Schema), plus `use_project` to pick or create the project. Calls run like the CLI's: inside the GUI that has the project open, or headless. `frame_render`, `gui_screenshot` and `storyboard_render` return images. Cancelling and progress: agents.md §1. To add it to Claude Code: `claude mcp add vidcut -- vidcut mcp -p /abs/talk.vidcut`; other clients take `{"command": "vidcut", "args": ["mcp", "-p", "/abs/talk.vidcut"]}`.

**Folders:** `~/.config/vidcut`, `~/.cache/vidcut` and `~/.local/state/vidcut` below are where vidcut keeps files on Linux; README.md's table says where on Windows (`%APPDATA%`, `%LOCALAPPDATA%`) and macOS (`~/Library`).

**Commands are the protocol's methods** (`crates/vidcut/src/cli.rs` generates the CLI from the request schema):
- Method `group.verb` is the command `vidcut group verb`: dots become spaces, underscores hyphens (`edit.place_on_top` is `vidcut edit place-on-top`; MCP calls it `edit_place_on_top`).
- Every parameter is the flag of its name: `volume_db` is `--volume-db`. A true/false parameter that defaults to false is a bare flag (`--lift`); one that may be left unset takes a value (`--locked true`). Lists take commas or repeated flags (`--at 1m,1m30s`). Objects take JSON inline, as `@FILE` or `-` for stdin.
- A command's main object (ids, files, the time to seek to) may go without its flag, in order: `vidcut media relink m1 new.mp4`.
- `vidcut schema` prints the whole protocol as JSON Schema; `vidcut schema edit.trim` prints one method's params. `vidcut --help` and `vidcut <group> <verb> --help` show the parameters and examples.
- Not requests: `vidcut` alone starts the GUI, `vidcut instances`, `vidcut mcp`, `vidcut rpc`.

**Basic use:**

```bash
vidcut project create talk.vidcut           # format from the first video added, or --fps 25 --width 1920 --height 1080
export VIDCUT_PROJECT=talk.vidcut           # or pass -p FILE every time
vidcut media add cam.mp4 screen.mkv --json  # -> media ids m1, m2
vidcut edit append m1 --range 10s..2m
vidcut edit split --at 1m,1m30s
vidcut edit delete V1@1m10s                 # the clip under 1m10s; ripples Track 1
vidcut timeline get --json                  # clips and gaps with frames and timecodes
vidcut history undo                         # works across separate runs
```

**Times:**
- In strings, every time needs a unit: `450f`, `15s`, `1m30s`, `250ms`, `00:01:00:00` (non-drop-frame timecode), `HH:MM:SS.mmm`, `start`, `end`, `in`, `out`.
- In JSON params, a bare integer is a frame number.
- Ranges are half-open: `A..B` excludes B; `A..+D` is D long.

**Clips:** refer to them by id (`c17`) or by position (`V1@<time>`, `V2@1m`).

**Tracks:**
- `vidcut track list` lists them. `track add video|audio [--count N]`, `track remove V3 [--force]` (`--force` also deletes its clips), and `track set A1 --volume-db -6 --name Music` with `--locked`, `--muted`, `--solo` and `--video-enabled`, each `true` or `false`.
- Track ids are positions: V1, V2, … from the bottom and A1, A2, … (spec D27). Removing a track renumbers the later ones, so list them again afterwards.
- Edit functions take `--track V2` (the target track) and `--audio-only`/`--video-only`. Naming the next new track (V3 when V2 is the last) creates it. Media without video go to A1 by default.
- `vidcut edit place-on-top m4 --range 5s..+3s` puts B-roll on the first track above everything at the playhead (or `--at`, or the timeline marks); the result names the clip and the track. `vidcut edit move c7 --to 2m --track V2` moves clips between tracks.
- Track 1 ripples. Clips on other tracks follow the Track 1 picture under their first frame (spec D21); other tracks ripple only where Track 1 is empty (D22). Only Track 1 pushes clips: inserts elsewhere overwrite (D23).
- Export, `frame render` and the preview show the highest visible clip and mix every audible track with its fader. Mute and solo apply to the export too (p. 650).

**Stills and image sequences (spec D38, D39):**
- `vidcut media add photo.jpg` adds a still. Edited without `--range` it lasts the project's still duration (5 s; `vidcut project set --still-duration 3s`); a range or trim makes it up to 17 h 40 min long.
- `vidcut media add renders/frame_000001.png --sequence [--fps 60]` adds the numbered files in a row around it as one clip (`frame_[000001-010560].png`), at the project's rate unless `--fps` is given. Without `--sequence` a numbered image is a still. Exporting a sequence fails if a frame's file is missing, rather than repeating the picture before it.
- `vidcut media get` reports `still: true`, or `sequence: {pattern, first, count}`.

**Proxies (spec D41, deviation X18):**
- `vidcut proxy generate [m1 m2…] [--width 960]` writes an 8-bit 4:2:0 proxy for each video or image sequence (default longest side 960; smaller pictures stay their size), with a keyframe every 60 frames and every frame coded in the order it shows. On the GPU: NVDEC decoding and scaling for NVENC (export.md §5); NVDEC, VA-API or Direct3D 11 decoding and shrinking for the other encoders (preview.md §6). The result's `made_with` says which (`decoded on the GPU (NVDEC), AV1, NVIDIA hardware (NVENC)`; `decoded on the GPU (VA-API), …`). Its codec is the preferences' `proxies.codec`: `auto` (the default) is AV1 on a hardware encoder (NVENC, then VAAPI) when one works here, else H.264 with x264; `av1` falls back to SVT-AV1 instead; `h264` is x264. `vidcut preferences get` says what each choice uses on this machine. The codec is in the file name, so changing it makes new proxies. With no ids, every such clip in the pool. Naming a still or an audio-only file is an error; those are skipped when generating everything. It shows progress like an export, Ctrl-C stops it without leaving a file, and the GUI and other `vidcut` runs aren't held up while it encodes. In a running GUI the encode is a job, like an export: the Jobs window shows its progress and Cancel, and `vidcut jobs list` lists it (`kind: "proxy"`).
- Automatic proxies (preferences `proxies.*`, File > Preferences): while `use_proxies` is on, a running GUI makes them by itself for image sequences, videos over 3840×2160 and files over 150 Mb/s, after an import and when proxies are turned on; these jobs show `origin: "session"`. Headless runs never do. `vidcut proxy generate --auto` takes only what those rules pick; the proxy size default is `proxies.size`.
- `vidcut proxy remove [m1…]` deletes the proxy files and clears them. `vidcut project set --use-proxies true` plays fresh proxies in the preview (the Media Pool has the same switch). Export, `frame render`, analysis and OTIO keep the original paths. Preview audio stays on the original file.
- Proxy frames keep the source's presentation timestamps, so swapping the preview path stays frame-exact. `vidcut media get` reports the proxy path and `stale: true` when the source size or modification time no longer matches. Files live in `proxies/` beside the project (or `~/.cache/vidcut/proxies` when the project path is unknown).

**Missing media (deviation X1):** `vidcut media list` marks media whose files are missing (`missing: true` in JSON).
- `vidcut media relink --search /mnt/footage` looks for all of them by file name there and in its subfolders, and relinks each file that is the same one (size, streams, fingerprint). `media relink m1,m4 --search DIR` looks for those only; `--dry-run` says what it would do.
- The result lists `relinked`, `different` (found by name, but another file, with `why`) and `not_found`. `vidcut media relink m1 new.mp4` uses that file either way, saying how it differs if it does.

**Bins:** `vidcut bins list` lists them. `bins add "Interviews/Day 1"` (parents too), `bins rename A B`, `bins remove A` (its media move up a level, deviation X6), `media add FILE --bin A`, `media move m1 m2 --bin A` (`""` is the top level). `media list` shows each item's bin.

**JSON output:**
- `--json` prints `{"rev", "changes": {"created", "modified", "deleted"}, "result", "via"}`. When the undo history could not be saved, `warnings` says so; the edit is in the file.
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

**Safe automation:**
- `--dry-run` reports `changes` without writing anything.
- `--if-rev N` fails with `CONFLICT` if someone else changed the project after you read revision `N`. (The project's `edited`, milliseconds since 1970 UTC, says when it last changed, for people: the GUI's status bar shows it. Compare `rev`, not times.) Every request on the project checks it: edits, exports, frames, measurements, queries, and live ones such as seeks and screenshots. An export encodes the project as it was when it started, whatever edits follow.
- `vidcut apply edits.jsonl` (or `-` for stdin) applies many requests atomically, as one undo step. The format is one `{"method", "params"}` object per line.
- Parallel `vidcut` processes on one project are serialized by a lock and never corrupt it.

**Raw requests:** `vidcut rpc edit.split '{"at": ["1m", 2700]}'` sends one exactly as written.

**Export and stills:**
- `vidcut export start out.mp4` renders the timeline: its In–Out range when both marks are set, else everything.
  - The extension picks the container, and also the preset unless `--preset` is given.
  - Extra options: `--preset youtube-1080p`, `--encoder nvenc|vaapi|amf|qsv|cpu|auto`, `--set video.quality=20`, `--range 1m..2m`, `--per-clip`, `--audio-only`, `--overwrite`.
  - With NVENC, VAAPI on the render node the pieces decode on, or AMF on their Direct3D 11 adapter (and `gpu.acceleration` on), frames are made on the GPU where the piece qualifies (export.md §5). The result's `gpu_frames` counts them.
  - With `gpu.acceleration` and `gpu.decoding` on, pieces decode on the GPU the viewer decodes on, whatever the encoder (export.md §5); `gpu_decoded` counts the frames decoded there.
  - `vidcut export plan out.mp4` (or `export start` with `--dry-run`) prints the plan and the chosen encoder without encoding.
  - Dynamic range and bit depth follow the footage (spec D44): `video.hdr` is `auto` (HDR when most of the shown picture is HDR and the codec carries it: HEVC, AV1, VP9), `sdr`, `pq` or `hlg`; an unset `video.bit_depth` is 10 for HDR, and for SDR when most of the picture has 10 bits or more (8 in H.264). Each output's `color` in the plan and the result says what was chosen and why (`hdr`, `bit_depth`, `picture`: frames of PQ, HLG and 10-bit picture, `why`). When HDR picture will be tone-mapped and SDR wasn't asked for, `notes` has `{"code": "hdr_tone_mapped", "message", "set"}`: `set` holds the `--set` overrides that keep it HDR (`video.codec=hevc`, `video.hdr=pq`), empty when no encoder the export may use writes it.
  - HDR metadata follows the footage too (spec D46): `video.hdr_metadata` is `auto` (PQ output gets the widest mastering display and the highest MaxCLL/MaxFALL of the sources, measured from the frames where a source has none; HDR10+ is kept when any picture has it and made for the frames without; Dolby Vision is kept when every picture has it), `static`, `hdr10plus`, `dolby_vision`, `all` or `none`. `--set video.mastering_display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1)` and `--set video.max_cll=1000,400` give the values (x265's forms); `video.max_cll=measure` measures every picture. Measuring happens when the export starts: the plan's `why` says `MaxCLL and MaxFALL: measured when the export starts`, and progress events and jobs carry `measuring: true` until it is done. Results are kept in `~/.cache/vidcut/analysis`. Each output's `metadata` in the plan and the result says what is written and why (`mode`, `mastering`, `light`, `why`). Dolby Vision is written by libx265 and libsvtav1, so `auto` then encodes on the CPU.
  - Passes (spec D45): with `--set video.bitrate=8000000 --set video.quality=null`, x264, x265 and VP9 encode twice (the first run measures, writing nothing) and NVENC looks at each frame twice in one encode; `video.passes` is `1` or `2` (unset: 2 with a bitrate, 1 with constant quality; two need a bitrate; VAAPI, AMF, QSV and SVT-AV1 make one). `video.first_pass=fast|full` (default fast), NVENC's `video.multipass=quarter|full` (default quarter) and `video.lookahead=N` (0–32, default 32). The plan's `video_encoder.passes` says how (`{"kind": "two_runs", "fast_first": true}`, `two_inside`, `one`), the result's `passes` how many; progress events and jobs carry `pass` and `passes`. The statistics sit in a hidden `.<name>.passes` folder beside the output while it runs.
  - Ctrl-C cancels cleanly (exit 9, no partial file).
  - With a GUI running, `--background` returns a job id at once; follow it with `vidcut jobs` commands (below).
- `vidcut presets list` lists the presets (`--json` includes their settings); `vidcut presets get NAME` shows one.
  - `vidcut presets save web --from youtube-1080p --set video.quality=22 [--quick-export]` saves a user preset to `~/.config/vidcut/presets/web.toml`; `--settings JSON|@FILE` gives full settings instead. `vidcut presets delete web` removes it.
- `vidcut encoders list` shows which GPU encoders work here (cached; `--refresh` re-tests), whether or not `gpu.acceleration` lets vidcut use them.
- `vidcut preferences get` shows the application preferences (`~/.config/vidcut/preferences.toml`) and what the thumbnail cache holds; `vidcut preferences set thumbnails.cache_limit_mb=256` changes them (`preferences.set` takes `{"thumbnails": {"disk_cache": false}}`; turning the cache off empties it). The cache holds the pool's pictures and the timeline's filmstrip tiles; `thumbnails.timeline_memory_mb` (256) is the memory a running GUI keeps tiles in. `player.memory_mb` and `player.video_memory_mb` are the viewer's for decoded pictures (`0`, the default, is automatic: player.md §3; `preferences get` says what `player_memory_auto_mb` comes to here); a running GUI takes a change at once.
  - `vidcut preferences set gpu.acceleration=false` (File > Preferences: GPU) turns off all GPU work (preview.md §3): exports and proxies take software encoders (`export plan` shows `cpu`), and the preview decodes in software at once. `player state`'s `hwdec` is `nvdec`, `vaapi`, `d3d11va` or `no`. On by default. `gpu.decoding=false` (the same window) decodes on the CPU alone, in the preview, exports, proxies and the timeline's pictures, the GPU only encoding: faster where the GPU is small and the CPU large (export.md §5).
  - `vidcut preferences set player.audio_device=<id>` (File > Preferences: Player, Sound) plays the preview's sound on that device, an id `vidcut doctor` lists under `audio_outputs` with its name and whether it is the default; empty for the system's default. The player switches at once, paused where it was. A device that isn't connected plays on the default: `player state`'s `audio_device` says which device plays (null: the system's default), and `audio_error` why it isn't the one chosen (not connected, or none opened).
- `vidcut frame render --at 1m --out f.png [--width 640]` renders what the timeline shows at a frame. `--media m1` renders a media frame instead. Use it to look at cuts.

**Automatic tools (spec D29–D32):**
- `vidcut edit ripple-delete-silence c1 [c2…] [--min 0.5s --threshold-db -40 --pre-head 0.15s --post-tail 0.15s]` (the defaults shown) cuts silent parts out of clips and closes the gaps, as one undo step. `--dry-run` lists the parts.
- `vidcut edit detect-scene-cuts c1` splits a clip where its shots change (`--threshold`, scene score 0–100, default 10).
- `vidcut analyze silence|scenes|black [--media m1 | --clips c1,c2]` and `vidcut analyze loudness [--range a..b]` measure without changing anything. Media positions are source frames; clip positions are timeline frames.
- `vidcut timeline boring [--boring 30s --jump 10f]` lists clips too long and too short (the Boring Detector).
- `vidcut storyboard render --out board.png [--every 5s --columns 4 --width 320]` draws a picture of every shot with timecode and clip id: the quickest way to look at a whole edit.
- `vidcut export start out.mp4 --set audio.loudness=-14` normalizes the mix to −14 LUFS (one gain, true peak ≤ −1 dBTP); the result reports the gain.
- `vidcut export start out.mp4 --set video.codec=hevc --set video.hdr=pq` (or `hlg`) exports HDR in 10 bits (HEVC, AV1 or VP9) whatever the footage; `video.hdr=sdr` tone-maps HDR sources to SDR. `vidcut media get` reports each clip's `hdr`, `rotation` and interlaced `fields`, and its HDR metadata: `mastering`, `light`, `hdr10plus`, `dolby_vision`.
- `vidcut edit slide c3 --by 5f` moves a clip keeping its media; its neighbors give and take.
- Measuring decodes a file once; the results are cached (`~/.cache/vidcut/analysis/`), so trying other settings is quick.

**Timeline interchange (OpenTimelineIO, deviation X9).** What the file contains is that deviation.
- `vidcut export start cut.otio` writes the whole timeline, or `--range`. `cut.otioz` bundles the media (a zip; in a GUI it runs as a job), `cut.otiod` the same as a folder. Tracks carry their names (`track set A1 --name Music`).
- `vidcut timeline import cut.otio` (or `.otioz`, or an `.otiod` folder) adds the file's media to the pool and the file's tracks after the last clip: video tracks onto V1, V2, … with the audio that belongs to their clips, other audio tracks onto A1, A2, …. `--replace` replaces the timeline instead. An empty timeline takes the file's frame rate, start timecode and track settings.
  - An `.otioz`'s media unpack next to it, into a folder named after it (`cut/media/…`), or into `--unpack DIR`. A dry run unpacks nothing and says what it would.
  - Media that moved: paths relative to the file and a project folder moved with the file are found by themselves; otherwise `--search /mnt/footage` looks for them by file name, subfolders too. Missing media fail the import (exit 7) with the list of files.
  - What can't be imported (transitions, compound clips, generators, speed changes, clip effects in use such as a zoom, crop or pan, clips overlapping others on their track) is listed in `warnings`.
- Raw: `timeline.import` also takes the document inline as `otio` (relative media paths then can't be found but by `search`).
- `tests/cli.rs` checks the files against OpenTimelineIO's own Python library when one is installed (`pip install opentimelineio`; `VIDCUT_OTIO_PYTHON` names the Python); otherwise that part skips.

**Live control of a running GUI** ([docs/decisions/gui.md](docs/decisions/gui.md)):
- **Routing:** when a GUI has the project open (`vidcut -p FILE` starts one), every project command runs inside it. You and the human then edit one live session: the GUI updates at once, and `via` in the JSON says `gui:<pid>`.
  - `--headless` forces a separate run; `--instance PID|SOCKET` (or `VIDCUT_SOCKET`) picks a GUI.
  - `vidcut instances` lists running GUIs.
- **Live-only commands:**

| Command | What |
|---|---|
| `vidcut player state` | viewer mode, both playheads and whether each one's picture is decoded (`ready`; the one not shown is kept ready, so switching shows it at once), the source clip, selection, what decodes (`hwdec`); at the top, the audio device (`audio_device`, `audio_error`), the viewer's late presents and skipped frames while playing (`late`, `skipped`) |
| `vidcut player seek 10s [--viewer timeline\|source_clip\|source_tape]` | returns once the frame is on screen (`CONFLICT` if someone moved the viewer first); `+10f`/`-2s` are relative |
| `vidcut player step [-]N`, `player play [--speed -2]`, `player pause`, `player toggle` | negative speeds play backwards, the sound reversed; other speeds keep its pitch |
| `vidcut viewer set source_clip\|source_tape\|timeline`, `viewer load m1 [--at 5s]` | what the viewer shows |
| `vidcut selection set --clips c1,c2 --media m3`, `selection get` | the selection |
| `vidcut gui screenshot --out v.png [--target window]` | the viewer picture as shown (works while minimized), or the whole window |
| `vidcut gui log [--since N]` | what everyone (the human too) did: each entry's `summary` in words, and the request as sent |
| `vidcut gui window --size 1600,1000`, `project open FILE [--create [--fps 25 --width 1920 --height 1080]]`, `gui quit` | window and project |
| `vidcut jobs list`, `jobs get\|wait\|cancel J [--timeout S]` | export and proxy jobs, the GUI's own included; `wait` fails like the job did |
| `vidcut pool set --bin A --sort name --descending true` | the bin the Media Pool shows and its order; Source Tape follows it |
| `vidcut player mute [true\|false]` | the viewer's monitoring mute (not the export); without a value, toggles |
| `vidcut player audio-scrubbing [true\|false]` | audio scrubbing (decision D42): steps and paused seeks play their frame's sound; without a value, toggles (Shift+S) |
| `vidcut mixer preview A1 -12` | a track's level heard now, without changing the project (`track set --volume-db` keeps one); `live: false` while the viewer shows the source |
| `vidcut gui show quick_export\|export\|jobs\|log\|mixer\|preferences` | opens that window for the person at the GUI |

- **Looking at the result:** `vidcut player seek 1m && vidcut gui screenshot --out shot.png` shows exactly what `vidcut frame render --at 1m --out f.png` renders. Both are frame-exact.
- **Exporting alongside the human:** `vidcut export start out.mp4 --background` returns at once; the export shows in the GUI's Jobs window, and `vidcut jobs wait j1` blocks until it's done. An export the human starts from Quick Export is a job too, so `vidcut jobs list` sees it.
- **Source marks and clipboard work headless too:**
  - `vidcut media marks set m1 --in 5s --out 10s`: edits without `--range` then use these marks.
  - Source Tape marks can span clips (spec D17): `vidcut tape marks set --in 200f --out 300f --order m1,m2` (tape frames of those media end to end), then edits with `--tape` instead of a media id, such as `vidcut edit append --tape`, place the span as one block.
  - `vidcut edit copy c3 --json`, then `vidcut edit paste --clips @clips.json --at 1m`.
- **Paths:** the CLI makes paths absolute before routing, those in `apply` batches too. In raw `rpc` requests, use absolute paths.
