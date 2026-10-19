# Driving vidcut from scripts and agents

Every command, with its parameters, defaults and possible values, is in `vidcut --help`, `vidcut <group> --help` and `vidcut <group> <verb> --help`. `vidcut schema` prints the protocol as JSON Schema, `vidcut schema edit.trim` one method. All are generated from the request schema.

```bash
vidcut project create talk.vidcut
export VIDCUT_PROJECT=talk.vidcut     # or -p talk.vidcut every time
vidcut media add cam.mp4 screen.mkv   # media m1 and m2
vidcut edit append m1 --range 10s..2m
vidcut edit split --at 1m,1m30s
vidcut timeline get --json
```

## Commands are requests

- The method `group.verb` is `vidcut group verb`: dots become spaces, underscores hyphens (`edit.place_on_top` is `vidcut edit place-on-top`).
- A parameter is the flag of its name (`volume_db` is `--volume-db`). A true/false parameter that defaults to false is a bare flag (`--lift`); others take a value (`--locked true`). Lists take commas or repeated flags (`--at 1m,1m30s`). Objects take JSON inline, `@FILE` or `-` (stdin).
- A command's main object (ids, files, the time to seek to) is positional, with no flag: `vidcut media relink m1 new.mp4`.
- `vidcut apply FILE` runs a batch ([Safe automation](#safe-automation)). `vidcut preferences set` takes `KEY=VALUE` pairs.
- `vidcut rpc METHOD [PARAMS]` sends one request as written: `vidcut rpc edit.split '{"at": "1m"}'`. Give it absolute paths.
- Not requests: `vidcut` alone (the GUI), `vidcut instances`, `vidcut mcp`.

## Global options

| Option | Does |
|---|---|
| `-p`, `--project FILE` | the project file; also `VIDCUT_PROJECT` |
| `--json` | prints the response as JSON, errors too, on stdout |
| `--dry-run` | reports what would change, changing nothing |
| `--if-rev REV` | fails with `CONFLICT` unless the project is at revision REV |
| `--headless` | runs here even if a GUI has the project open |
| `--instance PID\|SOCKET` | sends to this GUI; also `VIDCUT_SOCKET` |
| `--client NAME` | who the undo history says made the change, such as an agent's or a script's name; also `VIDCUT_CLIENT` |

## Times, ranges and clips

| Form | Means |
|---|---|
| `450f`, or a JSON integer | frames |
| `15s`, `250ms`, `1m30s`, `1h2m` | seconds, to the nearest frame |
| `01:00:00:00`, `01:00:00;00` | timecode from the project's start timecode; `;` is drop-frame |
| `00:01:02.500` | clock time |
| `start`, `end`, `in`, `out`, `playhead` | the timeline's ends, its marks, the playhead (GUI only) |
| `+10f`, `-2s` | relative, where a command allows (`player seek -2s`) |
| `A..B`, `A..+D` | half-open range: B excluded, or D long |
| `c17`, `V1@1m` | a clip by id, or the clip under that frame on a track |

A number without a unit is an error, except a `duration` of `marks set` and `media marks set`, which is frames, as in the viewer's Duration field. Ids: media `m1`, clips `c1`, markers `k1`, subtitle cues `q1`; tracks `V1`, `V2`, … from the bottom, `A1`, `A2`, … and subtitle tracks `S1`, `S2`, …, renumbered when one is removed. How they resolve: [design/model.md](design/model.md#time).

## Output

Without `--json`, an edit prints `ok edit.split (rev 7)` and the ids it changed, a query a summary or JSON. Warnings, errors and progress go to stderr.

With `--json`, stdout gets the response, empty fields left out:

| Field | Holds |
|---|---|
| `rev` | the revision after the request |
| `changes` | media, clip and marker ids `created`, `modified`, `deleted` |
| `result` | the method's result, untyped JSON ([Result fields](#result-fields)) |
| `via` | `headless` or `gui:<pid>` |
| `dry_run` | `true` for a dry run |
| `warnings` | problems that didn't fail it, such as an unsaved undo history |

An error is `{"error": {"code": "NOT_FOUND", "message": "…"}}` with `--json`. The exit code names it:

| Exit | Code | When |
|---|---|---|
| 1 | `INTERNAL` | a vidcut bug |
| 2 | `USAGE` | bad command, parameter or time |
| 3 | `NOT_FOUND` | no such id, track, preset, job or project |
| 4 | `INVALID_EDIT` | the edit can't be made |
| 5 | `CONFLICT` | `--if-rev` failed, the project is locked, or another seek came first |
| 6 | `NO_INSTANCE` | needs a running GUI |
| 7 | `MEDIA` | media missing or unreadable |
| 8 | `EXPORT` | encoding or writing failed |
| 9 | `CANCELLED` | Ctrl-C or `jobs cancel` |
| 10 | `DEPENDENCY` | FFmpeg lacks a part (`vidcut doctor`), or a speech model isn't downloaded yet (`--download`) |

## Safe automation

- `--dry-run` reports the result and `changes` and saves nothing. `export start --dry-run` is `export plan`.
- `--if-rev N` fails with `CONFLICT` unless the project is still at `N`, the `rev` you read; every request on a project checks it.
- `vidcut apply FILE` (`-`: stdin) runs requests in order on one copy of the project: one revision, one undo step (`--label` names it). Any error fails them all.
  - One `{"method": …, "params": {…}}` per line (blank and `//` lines skipped), or one JSON array.
  - `apply` and the `history` commands that change the history can't be batched; `--if-rev` and `--dry-run` go on the command line.
- Processes editing one project take its lock in turn ([design/requests.md](design/requests.md)).

## Live control of a running GUI

When a GUI has the project open (`vidcut -p FILE` starts one), commands for it run inside that GUI, where the person sees each edit. Routing: [design/requests.md](design/requests.md#routing); the GUI's side: [design/live.md](design/live.md).

- Without `-p`, a command goes to the only GUI with a project open; with several, pick one.
- `vidcut instances` lists running GUIs: pid, project, socket.
- Requests needing no project (`schema`, `doctor`, `reset`, `probe`, `project create`, presets, encoders, decoders, preferences, keys) run here.
- Live-only requests (`player`, `viewer`, `selection`, `pool`, `mixer preview`, `gui`, `jobs`, `project open`) and `export start --background` fail with `NO_INSTANCE` without a GUI.
- The CLI makes paths absolute before sending, those in `apply` too.
- Ctrl-C cancels a headless export, proxy encode or transcription, leaving no partial file but a model download's `.part`, which the next download goes on from; ending the CLI cancels its call to a GUI.
- Other programs may speak JSON-RPC 2.0 to a GUI's socket, one request per line.

## MCP

`vidcut mcp` serves the Model Context Protocol on stdin/stdout ([deviation X10](deviations.md#x10-an-mcp-server-for-ai-agents)). With Claude Code:

```bash
claude mcp add vidcut -- vidcut mcp -p /abs/talk.vidcut
```

Other clients: `{"command": "vidcut", "args": ["mcp", "-p", "/abs/talk.vidcut"]}`.

- Each method but `schema` is a tool, named with underscores (`edit_split`). `use_project` picks a project, or with `create: true` makes one.
- Calls route like the CLI's; arguments may carry `dry_run` and `if_rev`.
- Results are the response JSON; failures are tool errors naming the error code.
- `frame_render`, `gui_screenshot` and `storyboard_render` also return the PNG as an image.

## Result fields

The schema describes parameters, not results. Commonly read:

| Request | Fields |
|---|---|
| `apply` | `results`, in order |
| `export start` | `outputs[]`: `frames`, `video_encoder`, `gpu_frames` (made on the GPU), `gpu_decoded`, `copies` (GPU frames copied `down` and `up`), `color` and `metadata` (what was chosen, and `why`), `tried` (video encoders that didn't open, and why), `sources` (each file and how it was decoded). `notes[]`: `code`, `message` |
| `export start --background`, `jobs get` | `id`, `state` (`queued`, `running`, `done`, `failed`, `cancelled`), `progress` (0–1), `result`, `error`, and `details` as it runs: `video` (`encoder`, `device`, `pixel`, `bits`, `passes`, `gpu_frames_in`), `audio`, `color`, `metadata`, `tried`, `sources`, `downloading` (the file being downloaded), `gpu_frames`, `gpu_decoded`, `copied`, `fps` |
| `proxy generate` | `proxies[]`: `media`, `path`, `made_with` (how it was decoded and encoded), `copies` |
| `titles add` | `clip`, `track` (where it went) |
| `titles list` | per title: `clip`, `track`, `start`, `end` (frames), `start_tc`, `end_tc`, `enabled`, `title` |
| `subtitles export` | `path`, `track`, `format`, `cues` (how many), `warnings` (an IMSC1 file's cues with more lines than their region holds) |
| `subtitles transcribe` | `track`, `cues` (their ids), `from`, `to`, `heard` (the tracks heard that have sound), `model`, `device` (the GPU or `CPU`), `language`, `timing` (`words`, `phrases`, or `pauses` for a model without times), `pieces`, `longest_piece` (seconds), `warnings` |
| `analyze words` | `words[]`: `text`, `start`, `end` (timeline frames), `joined` (no space before it), `filler` (a filler word in `filler_language`). The rest as `subtitles transcribe`, without cues. A model that doesn't time words is an error. With `--media` (and `--stream`, else the first stream transcribed) or `--clips`, one entry for each: `media` or `clip` and `track`, `words[]` (source frames for a media), `model`, `language` |
| `media transcribe` | `media[]`: `media`, `stream`, `model`, `device`, `language`, `timing`, `words` (how many), `warnings` |
| `media leave-out` | `left_out`: the media's spans left out, after the change |
| `edit ripple-delete-words` | `removed[]`: `start`, `end` (the spans taken out, touching ones joined), `frames` |
| `subtitles retime` | `track`, `cues`, `heard` (the tracks heard that have sound), `moved`, `removed` (cues that would end before zero), `speech` (`offset`, `speed`, `ratio`, `peak`, `rival`, `rival_offset`, `clarity`, `clear`, `needed`), `words` with `--words` (`model`, `device`, `language`, `matched`, `unmatched` (cue ids), `speech_errors`, `levels[]`), `correction` (`kind`: `offset`, `speed`, `sections` or `words`; `offset`, `speed`, `ratio`, `sections[]`, `snapped`, `errors`), `warnings`. Errors are `median`, `p90` and `within` (the share within 0.5 s), in seconds, over the matched cues |
| `media list`, `media get` | `missing` (the file is gone), `proxy`: `file`, `path` (in this computer's proxy folder), `width`, `height`, `codec`, `stale` (made from another version of the file or with another `color_space`), `missing` (its file is gone); `transcripts[]`: `stream`, `model`, `language`, `timing`, `words`; `left_out` (source spans the Transcription window leaves out) |
| `media add-folder` | `bin` (the folder's), `media` (in the folder's order), `skipped[]`: `path`, `why` |
| `media relink` | `relinked`, `different` (with `why`), `not_found` |
| `player state` | per viewer (`timeline`, `source`): `position`, `playing`, `ready`, `hwdec` (the GPU decoder, or `no`). `late`, `skipped` (frames while playing), `audio_device` (null: the default), `audio_error`, `loop` (Loop playback is on), `two_up` (the trim two-up's frames) |
