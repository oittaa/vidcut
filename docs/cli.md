# Driving vidcut from scripts and agents

**MCP** (deviation X10, [decisions/agents.md](decisions/agents.md)): `vidcut mcp [-p FILE]` serves the Model Context Protocol on stdin/stdout. Every command is a tool (`edit.split` is `edit_split`), plus `use_project` to pick or create the project; `frame_render`, `gui_screenshot` and `storyboard_render` return images.
- Claude Code: `claude mcp add vidcut -- vidcut mcp -p /abs/talk.vidcut`; other clients: `{"command": "vidcut", "args": ["mcp", "-p", "/abs/talk.vidcut"]}`.

**Folders:** `~/.config/vidcut`, `~/.cache/vidcut` and `~/.local/state/vidcut` below are the Linux ones; README.md gives Windows and macOS.

**Commands are the protocol's methods**, generated from the request schema:
- Method `group.verb` is `vidcut group verb`, underscores becoming hyphens: `edit.place_on_top` is `vidcut edit place-on-top`.
- Parameters are flags of their names (`volume_db` is `--volume-db`). A boolean that defaults to false is a bare flag (`--lift`); one that may be unset takes a value (`--locked true`). Lists take commas or repeated flags (`--at 1m,1m30s`); objects take JSON inline, as `@FILE` or `-` for stdin.
- A command's main object (ids, files, the time to seek to) may go without its flag, in order: `vidcut media relink m1 new.mp4`.
- `vidcut schema [edit.trim]` prints the protocol, or one method's params, as JSON Schema; `--help` on any command shows its parameters and examples.
- `vidcut rpc edit.split '{"at": ["1m", 2700]}'` sends a raw request exactly as written. Not requests themselves: `vidcut` alone (the GUI), `vidcut instances`, `vidcut mcp`, `vidcut rpc`.

**Basic use:**

```bash
vidcut project create talk.vidcut           # format from the first video added, or --fps 25 --width 1920 --height 1080; --start-tc 01:00:00:00
export VIDCUT_PROJECT=talk.vidcut           # or pass -p FILE every time
vidcut media add cam.mp4 screen.mkv --json  # -> media ids m1, m2
vidcut edit append m1 --range 10s..2m
vidcut edit split --at 1m,1m30s
vidcut edit delete V1@1m10s                 # the clip under 1m10s; ripples Track 1
vidcut timeline get --json                  # clips and gaps with frames and timecodes
vidcut history undo                         # works across separate runs
```

**Times and clips:**
- In strings, every time needs a unit: `450f`, `15s`, `1m30s`, `250ms`, `00:01:00:00` (timecode; `00:01:00;00` drop-frame, decision D47), `HH:MM:SS.mmm`, `start`, `end`, `in`, `out`. In JSON params, a bare integer is a frame number.
- Ranges are half-open: `A..B` excludes B; `A..+D` is D long. Clips are named by id (`c17`) or by position (`V1@<time>`, `V2@1m`).

**Tracks** (spec D21–D27):
- `vidcut track list`; `track add video|audio [--count N]`; `track remove V3 [--force]` (`--force` also deletes its clips); `track set A1 --volume-db -6 --name Music`, with `--locked`, `--muted`, `--solo` and `--video-enabled` taking `true` or `false`.
- Track ids are positions (V1, V2, … from the bottom; A1, A2, …): removing a track renumbers the later ones. Edit functions take `--track V2` (naming the next new track creates it) and `--audio-only`/`--video-only`; media without video go to A1.
- `vidcut edit place-on-top m4 --range 5s..+3s` puts B-roll on the first track above everything at the playhead (or `--at`, or the timeline marks); the result names the clip and the track. `vidcut edit move c7 --to 2m --track V2` moves clips between tracks.
- Track 1 ripples and pushes clips; other tracks follow it (D21–D23). Export, `frame render` and the preview show the highest visible clip and mix every audible track; mute and solo apply to the export too.

**Media** (spec D38, D39):
- `vidcut media add photo.jpg` adds a still, lasting the project's still duration (`project set --still-duration 3s`) unless given a range.
- `vidcut media add renders/frame_000001.png --sequence [--fps 60]` adds the numbered run around it as one clip (`frame_[000001-010560].png`). Exporting fails if a frame's file is missing.
- `vidcut media get` reports `still: true` or `sequence: {pattern, first, count}`, `hdr`, `rotation`, interlaced `fields`, and HDR metadata (`mastering`, `light`, `hdr10plus`, `dolby_vision`).

**Proxies** (spec D41, deviation X18):
- `vidcut proxy generate [m1 m2…] [--width 960] [--auto]` makes proxies for videos and image sequences: every one without ids, only those the automatic rules pick with `--auto`. The preferences' `proxies.*` set the rules, the size and the codec (`proxies.codec`: `auto`, `av1`, `h264`); the result's `made_with` says what decoded and encoded it.
- In a running GUI the encode is a job (`kind: "proxy"`; `origin: "session"` for those it makes by itself). `vidcut proxy remove [m1…]` deletes proxies. `vidcut project set --use-proxies true` plays fresh proxies in the preview.
- `vidcut media get` reports the proxy path, and `stale: true` when the source changed.

**Missing media** (deviation X1): `vidcut media list` marks them `missing: true`.
- `vidcut media relink --search /mnt/footage` looks for all of them by file name there and in subfolders, relinking each that is the same file; `media relink m1,m4 --search DIR` looks for those only; `--dry-run` says what it would do.
- The result lists `relinked`, `different` (another file of that name, with `why`) and `not_found`. `vidcut media relink m1 new.mp4` uses that file either way.

**Bins:** `vidcut bins list`, `bins add "Interviews/Day 1"` (parents too), `bins rename A B`, `bins remove A` (its media move up, deviation X6), `media add FILE --bin A`, `media move m1 m2 --bin A` (`""` is the top level). `media list` shows each item's bin.

**JSON output:**
- `--json` prints `{"rev", "changes": {"created", "modified", "deleted"}, "result", "via"}`, with `warnings` when the undo history could not be saved (the edit is in the file).
- Errors print `{"error": {"code", "message"}}` and exit with its code: 1 `INTERNAL`, 2 `USAGE`, 3 `NOT_FOUND`, 4 `INVALID_EDIT`, 5 `CONFLICT`, 6 `NO_INSTANCE`, 7 `MEDIA`, 8 `EXPORT`, 9 `CANCELLED`, 10 `DEPENDENCY`. Success exits 0.

**FFmpeg's parts:** `vidcut doctor` lists the parts vidcut uses that this FFmpeg build lacks (`ffmpeg_missing`: each one's name, kind, library and use). An export that needs one fails with `DEPENDENCY` before writing anything.

**Safe automation:**
- `--dry-run` reports `changes` without writing anything. `--if-rev N` fails with `CONFLICT` if the project changed after you read revision `N`; every request on the project checks it, live ones too.
- The project's `edited` (ms since 1970 UTC) is for people; compare `rev`. An export encodes the project as it was when it started.
- `vidcut apply edits.jsonl` (or `-` for stdin) applies one `{"method", "params"}` object per line atomically, as one undo step. Parallel `vidcut` processes on one project are serialized by a lock.

**Export:**
- `vidcut export start out.mp4` renders the In–Out range when both marks are set, else everything; the extension picks the container, and the preset unless `--preset` is given. Ctrl-C cancels cleanly (exit 9, no partial file). With a GUI running, `--background` returns a job id at once (`vidcut jobs`, below).
  - Options: `--preset youtube-1080p`, `--encoder nvenc|vaapi|amf|qsv|cpu|auto`, `--set video.quality=20`, `--range 1m..2m`, `--per-clip`, `--audio-only`, `--overwrite`.
  - `vidcut export plan out.mp4` (or `export start --dry-run`) prints the plan and the chosen encoder without encoding. The result's `gpu_frames` and `gpu_decoded` count frames made and decoded on the GPU ([export.md](decisions/export.md) §5).
  - Dynamic range and bit depth (spec D44): `video.hdr` is `auto`, `sdr`, `pq` or `hlg`; an unset `video.bit_depth` follows the footage. Each output's `color` (plan and result) gives `hdr`, `bit_depth`, `picture`, `why`; tone-mapped HDR adds `notes` `{"code": "hdr_tone_mapped", "message", "set"}`, `set` holding overrides that keep it HDR.
  - HDR metadata (spec D46): `video.hdr_metadata` is `auto`, `static`, `hdr10plus`, `dolby_vision`, `all` or `none`. `video.mastering_display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1)` and `video.max_cll=1000,400` (x265's forms) set values; `video.max_cll=measure` measures every picture.
  - Each output's `metadata` in the plan and result gives `mode`, `mastering`, `light` and `why`; progress events and jobs carry `measuring: true` while light levels are measured.
  - Passes (spec D45): `video.passes` is `1` or `2` (two need `video.bitrate` with `video.quality=null`); `video.first_pass=fast|full`; NVENC's `video.multipass=quarter|full` and `video.lookahead=N` (0–32).
  - The plan's `video_encoder.passes` is `{"kind": "two_runs", "fast_first": true}`, `two_inside` or `one`; the result has `passes`, progress events and jobs `pass` and `passes`.
  - `--set audio.loudness=-14` normalizes the mix to −14 LUFS (true peak ≤ −1 dBTP); the result reports the gain.
- `vidcut presets list [--json]` and `presets get NAME` show presets. `presets save web --from youtube-1080p --set video.quality=22 [--quick-export]` writes `~/.config/vidcut/presets/web.toml`, or `--settings JSON|@FILE` gives full settings; `presets delete web` removes it.
- `vidcut encoders list [--refresh]` shows which GPU encoders work here (cached).
- `vidcut frame render --at 1m --out f.png [--width 640]` renders what the timeline shows at a frame; `--media m1` renders a media frame instead.

**Preferences** (`~/.config/vidcut/preferences.toml`, deviation X12): `vidcut preferences get` shows them, what the thumbnail cache holds and what automatic choices come to here. `vidcut preferences set thumbnails.cache_limit_mb=256` changes them (`preferences.set` takes `{"thumbnails": {"disk_cache": false}}`), at once in a running GUI.
- `thumbnails.disk_cache`, `thumbnails.cache_limit_mb`, `thumbnails.timeline_memory_mb` (256): the thumbnail cache and the GUI's timeline tile memory.
- `player.memory_mb`, `player.video_memory_mb`: the viewer's memory for decoded pictures; `0`, the default, is automatic ([player.md](decisions/player.md) §3), reported as `player_memory_auto_mb`.
- `gpu.acceleration=false` moves all GPU work to the CPU ([preview.md](decisions/preview.md) §3); `gpu.decoding=false` only the decoding (export.md §5). `player state`'s `hwdec` is `nvdec`, `vaapi`, `d3d11va` or `no`.
- `player.audio_device=<id>`: an id from `vidcut doctor`'s `audio_outputs`, empty for the system default. `player state` reports `audio_device` (null: the default) and `audio_error`.

**Automatic tools** (spec D29–D32):
- `vidcut edit ripple-delete-silence c1 [c2…] [--min 0.5s --threshold-db -40 --pre-head 0.15s --post-tail 0.15s]` (the defaults) cuts silent parts out of clips and closes the gaps, as one undo step; `--dry-run` lists the parts.
- `vidcut edit detect-scene-cuts c1 [--threshold 10]` splits a clip where its shots change (scene score 0–100).
- `vidcut analyze silence|scenes|black [--media m1 | --clips c1,c2]` and `vidcut analyze loudness [--range a..b]` measure without changing anything, in source frames for media and timeline frames for clips. Results are cached in `~/.cache/vidcut/analysis/`.
- `vidcut timeline boring [--boring 30s --jump 10f]` lists clips too long and too short; `vidcut storyboard render --out board.png [--every 5s --columns 4 --width 320]` draws every shot with timecode and clip id.
- `vidcut edit slide c3 --by 5f` moves a clip keeping its media; its neighbors give and take.

**Timeline interchange** (OpenTimelineIO, deviation X9):
- `vidcut export start cut.otio` writes the whole timeline, or `--range`; `cut.otioz` bundles the media (a job in a GUI), `cut.otiod` as a folder.
- `vidcut timeline import cut.otio` (or `.otioz`, or an `.otiod` folder) adds the media to the pool and the tracks after the last clip; `--replace` replaces the timeline.
  - An `.otioz` unpacks next to itself into `cut/media/…`, or into `--unpack DIR`; a dry run unpacks nothing.
  - `--search /mnt/footage` finds moved media by file name, subfolders too. Missing media fail the import (exit 7) with the list of files. Raw `timeline.import` also takes the document inline as `otio`, its relative paths then found only by `search`.
  - `warnings` lists what couldn't be imported. `timeline get` lists clip `effects` and each track's `transitions`.

**Live control of a running GUI** ([decisions/gui.md](decisions/gui.md)):
- **Routing:** when a GUI has the project open (`vidcut -p FILE` starts one), every project command runs inside it, and `via` in the JSON says `gui:<pid>`. `--headless` forces a separate run; `--instance PID|SOCKET` (or `VIDCUT_SOCKET`) picks a GUI; `vidcut instances` lists them.

| Live-only command | What |
|---|---|
| `vidcut player state` | viewer mode, both playheads and whether each one's picture is decoded (`ready`), the source clip, selection, what decodes (`hwdec`); at the top, the audio device (`audio_device`, `audio_error`), late presents and skipped frames while playing (`late`, `skipped`) |
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
| `vidcut player audio-scrubbing [true\|false]` | audio scrubbing (decision D42); without a value, toggles (Shift+S) |
| `vidcut mixer preview A1 -12` | a track's level heard now, without changing the project; `live: false` while the viewer shows the source |
| `vidcut gui show quick_export\|export\|jobs\|log\|mixer\|preferences` | opens that window for the person at the GUI |

- **Looking at the result:** `vidcut player seek 1m && vidcut gui screenshot --out shot.png` shows exactly what `vidcut frame render --at 1m --out f.png` renders.
- **Headless too:** `vidcut media marks set m1 --in 5s --out 10s` sets the marks edits without `--range` use. Source Tape marks can span clips (spec D17): `vidcut tape marks set --in 200f --out 300f --order m1,m2`, then `--tape` instead of a media id (`vidcut edit append --tape`).
- **Clipboard:** `vidcut edit copy c3 --json`, then `vidcut edit paste --clips @clips.json --at 1m`.
- **Paths:** the CLI makes paths absolute before routing, in `apply` batches too; raw `rpc` requests need absolute paths.
