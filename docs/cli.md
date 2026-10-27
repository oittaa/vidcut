# Driving vidcut from scripts and agents

- `vidcut --help`, `vidcut <group> --help` and `vidcut <group> <verb> --help` give each command, with its parameters, defaults and possible values.
- `vidcut schema` prints the protocol as JSON Schema. `vidcut schema edit.trim` prints one method.
- vidcut makes all of them from the request schema.

```bash
vidcut project create talk.vidcut
export VIDCUT_PROJECT=talk.vidcut     # or -p talk.vidcut every time
vidcut media add cam.mp4 screen.mkv   # media m1 and m2
vidcut edit append m1 --range 10s..2m
vidcut edit split --at 1m,1m30s
vidcut timeline get --json
```

## Commands are requests

- The method `group.verb` is `vidcut group verb`. Dots become spaces, and underscores become hyphens: `edit.place_on_top` is `vidcut edit place-on-top`.
- A parameter is the flag of its name: `volume_db` is `--volume-db`.
  - A true/false parameter with the default false is a flag without a value (`--lift`). Other parameters take a value (`--locked true`).
  - Lists take commas or repeated flags (`--at 1m,1m30s`).
  - Objects take JSON inline, `@FILE` or `-` (stdin).
- The main object of a command (ids, files, the time to seek to) is positional, with no flag: `vidcut media relink m1 new.mp4`.
- `vidcut apply FILE` runs a batch ([Safe automation](#safe-automation)).
- `vidcut preferences set` takes `KEY=VALUE` pairs.
- `vidcut rpc METHOD [PARAMS]` sends one request as you write it: `vidcut rpc edit.split '{"at": "1m"}'`. Give it absolute paths.
- These are not requests: `vidcut` alone (the GUI), `vidcut instances` and `vidcut mcp`.

## Global options

| Option | Does |
|---|---|
| `-p`, `--project FILE` | the project file; also `VIDCUT_PROJECT` |
| `--json` | prints the response as JSON on stdout, errors too |
| `--dry-run` | reports what would change, and changes nothing |
| `--if-rev REV` | fails with `CONFLICT` unless the project is at revision REV |
| `--headless` | runs in the CLI process, even if a GUI has the project open |
| `--instance PID\|SOCKET` | sends to this GUI; also `VIDCUT_SOCKET` |
| `--client NAME` | the name that the undo history gives as the author of the change, for example the name of an agent or a script; also `VIDCUT_CLIENT` |

## Times, ranges and clips

| Form | Means |
|---|---|
| `450f`, or a JSON integer | frames |
| `15s`, `250ms`, `1m30s`, `1h2m` | seconds, to the nearest frame |
| `01:00:00:00`, `01:00:00;00` | timecode from the start timecode of the project; `;` is drop-frame |
| `00:01:02.500` | clock time |
| `start`, `end`, `in`, `out`, `playhead` | the ends of the timeline, its marks, the playhead (GUI only) |
| `+10f`, `-2s` | relative, where a command allows it (`player seek -2s`) |
| `A..B`, `A..+D` | a half-open range: B is not in it, or the range is D long |
| `c17`, `V1@1m` | a clip by id, or the clip under that frame on a track |

- A number without a unit is an error.
  - The exception is a `duration` of `marks set` and `media marks set`. It is in frames, as in the Duration field of the viewer.
- Ids: media `m1`, clips `c1`, markers `k1`, subtitle cues `q1`.
- Tracks: `V1`, `V2`, … from the bottom, `A1`, `A2`, … and subtitle tracks `S1`, `S2`, ….
  - When you remove a track, vidcut renumbers the tracks.
- How they resolve: [design/model.md](design/model.md#time).

## Output

- Without `--json`, an edit prints `ok edit.split (rev 7)` and the ids that it changed. A query prints a summary or JSON.
- Warnings, errors and progress go to stderr.

With `--json`, stdout gets the response, without the empty fields:

| Field | Holds |
|---|---|
| `rev` | the revision after the request |
| `changes` | media, clip and marker ids `created`, `modified`, `deleted` |
| `result` | the result of the method, untyped JSON ([Result fields](#result-fields)) |
| `via` | `headless` or `gui:<pid>` |
| `dry_run` | `true` for a dry run |
| `warnings` | problems that did not make the request fail, for example an unsaved undo history |

With `--json`, an error is `{"error": {"code": "NOT_FOUND", "message": "…"}}`. The exit code tells which error it is:

| Exit | Code | When |
|---|---|---|
| 1 | `INTERNAL` | a vidcut bug |
| 2 | `USAGE` | a bad command, parameter or time |
| 3 | `NOT_FOUND` | no such id, track, preset, job or project |
| 4 | `INVALID_EDIT` | the edit cannot be made |
| 5 | `CONFLICT` | `--if-rev` failed, the project is locked, or another seek came first |
| 6 | `NO_INSTANCE` | the request needs a running GUI |
| 7 | `MEDIA` | media is missing or cannot be read |
| 8 | `EXPORT` | encoding or writing failed |
| 9 | `CANCELLED` | Ctrl-C or `jobs cancel` |
| 10 | `DEPENDENCY` | FFmpeg does not have a part (`vidcut doctor`), a speech model is not downloaded yet (`--download`), or audio would go to a paid service without `--send` |

## Safe automation

- `--dry-run` reports the result and `changes`, and saves nothing. `export start --dry-run` is `export plan`.
- `--if-rev N` fails with `CONFLICT` unless the project is still at `N`, the `rev` that you read. Each request on a project checks it.
- `vidcut apply FILE` (`-`: stdin) runs requests in order on one copy of the project.
  - The requests make one revision and one undo step. `--label` names the step.
  - Any error fails them all.
  - Write one `{"method": …, "params": {…}}` on each line, or one JSON array. vidcut skips blank lines and `//` lines.
  - You cannot batch `apply`, or the `history` commands that change the history.
  - `--if-rev` and `--dry-run` go on the command line.
- Processes that edit one project take its lock in turn ([design/requests.md](design/requests.md)).
- The paid speech models (`--model xai`, `--model google`) upload only with `--send`. Without it, the error gives the length, the size and the estimated cost.
- `vidcut services set-key --service xai` reads the key from standard input. Thus the key stays out of the history of the shell.

## Live control of a running GUI

When a GUI has the project open (`vidcut -p FILE` starts one), the commands for that project run inside that GUI. The person at the GUI sees each edit.

- Routing: [design/requests.md](design/requests.md#routing). The side of the GUI: [design/live.md](design/live.md).
- Without `-p`, a command goes to the only GUI with a project open. When there are several, select one.
- `vidcut instances` lists the running GUIs: pid, project, socket.
- The requests that need no project run in the CLI process: `schema`, `doctor`, `reset`, `probe`, `project create`, presets, looks, encoders, decoders, preferences and keys.
- Without a GUI, these requests fail with `NO_INSTANCE`:
  - The live-only requests: `player`, `viewer`, `selection`, `pool`, `mixer preview`, `gui`, `jobs` and `project open`.
  - `export start --background`.
- The CLI makes paths absolute before it sends them. This includes the paths in `apply`.
- Ctrl-C cancels a headless export, a proxy encode or a transcription.
  - It leaves no partial file, except the `.part` file of a model download. The next download continues from that file.
- When the CLI ends, it cancels its call to a GUI.
- Other programs can speak JSON-RPC 2.0 to the socket of a GUI, one request on each line.

## MCP

`vidcut mcp` serves the Model Context Protocol on stdin/stdout ([deviation X10](deviations.md#x10-an-mcp-server-for-ai-agents)). With Claude Code:

```bash
claude mcp add vidcut -- vidcut mcp -p /abs/talk.vidcut
```

Other clients: `{"command": "vidcut", "args": ["mcp", "-p", "/abs/talk.vidcut"]}`.

- Each method except `schema` is a tool, with underscores in its name (`edit_split`).
- `use_project` selects a project. With `create: true`, it makes one.
- Calls route like the calls of the CLI. Arguments can carry `dry_run` and `if_rev`.
- Results are the response JSON. Failures are tool errors that give the error code.
- `frame_render`, `gui_screenshot` and `storyboard_render` also return the PNG as an image.

## Result fields

The schema describes the parameters, not the results. This list gives the result fields that are commonly read.

- `apply`: `results`, in order.
- `export start`:
  - `outputs[]`:
    - `frames`, `video_encoder` and `gpu_decoded`.
    - `gpu_frames`: the frames made on the GPU.
    - `copies`: the GPU frames copied `down` and `up`.
    - `through_memory`: why the frames went to a GPU encoder through memory, when they did.
    - `color` and `metadata`: what was chosen, and `why`.
    - `tried`: the video encoders that did not open, and why.
    - `sources`: each file, and how it was decoded.
      - `went`: how the decoded frames of the file went to the draw: `frames stayed there` or `frames came down whole`.
      - `went` is not there when the frames of the file come to the draw in memory.
  - `upscaled[]`: each file that an AI upscaler of Super Scale enlarged (`path`), and `how`: the upscaler and its model.
  - `notes[]`: `code`, `message`.
- `export plan`:
  - `settings`, `video_encoder` and `notes`.
  - `outputs[]`, with `color` and `metadata`.
  - `auto`: what each automatic setting picks when only that setting is on auto.
    - It has a list for each of `encoder`, `passes`, `bit_depth`, `hdr` and `metadata`.
    - Each list has one entry for each different pick across the outputs.
- `export start --background`, `jobs get`:
  - `id` and `state` (`queued`, `running`, `done`, `failed`, `cancelled`).
  - `progress` (0–1), `result` and `error`.
  - `details`, while the job runs:
    - `video`: `encoder`, `device`, `pixel`, `bits`, `passes`, `gpu_frames_in`.
    - `audio`, `color`, `metadata`, `tried`, `sources` and `upscaled`.
    - `downloading`: the file that is being downloaded.
    - `speech`: the `model` and `find` of a transcription. Also `device`, `language` and `speakers_by`, when each is known.
    - `gpu_frames`, `gpu_decoded` and `copied`.
    - `fps`, only while the job encodes.
- `proxy generate`: `proxies[]`: `media`, `path`, `made_with` (how it was decoded and encoded), `copies`.
- `titles add`: `clip` and `track` (where the title went).
- `looks list`:
  - `looks[]`:
    - `name`, `kind` (`subtitles` or `titles`) and `built_in`.
    - `description`: the description of a built-in look.
    - `subtitles` or `titles`: the look, in the form that `looks save` takes.
  - `refused[]`: `kind`, `name`, `path`, `why` (the look files that were left out).
- `looks save`: `look` and `path` (the file written).
- `looks delete`: `deleted`, `kind` and `path`.
- `titles list`: for each title, `clip`, `track`, `start`, `end` (frames), `start_tc`, `end_tc`, `enabled` and `title`.
- `subtitles export`:
  - `path`, `track`, `format` and `cues` (how many).
  - `warnings`: the cues of an IMSC1 file that have more lines than their region holds.
- `subtitles transcribe`:
  - `track`, `cues` (their ids), `from` and `to`.
  - `heard`: the tracks heard that have sound.
  - `hear`: `media`, `mix` or `transcript`.
  - `media[]`, when `hear` is `media` or `transcript`. Each entry has `media` and `stream`.
    - Each entry also has the `model`, `device`, `language`, `words` (how many) and `speakers_by` of the transcript that its words came from.
  - `model` and `device`: the device is the GPU or `CPU`. With `media[]`, they are the ones that made the transcripts.
  - `language`.
  - `timing`: `words`, `phrases`, or `pauses` for a model without times.
  - `find`: `voice` or `sound`.
  - `speakers_by`: the speaker models that told the speakers, when you ask for speakers.
  - `pieces`, `longest_piece` (seconds), `warnings` and `notes`.
- `analyze words`:
  - `words[]`: `text`, `start`, `end` (timeline frames), `joined` (no space before it), `filler` (a filler word in `filler_language`).
  - `filler_words`: the list used, from `fillers_from`: `project`, `preferences` or `built_in`.
  - The other fields are as in `subtitles transcribe`, without cues.
  - A model that does not time words is an error.
  - With `--media` or `--clips`, there is one entry for each media or clip:
    - `media`, or `clip` and `track`.
    - `words[]`: in source frames for a media.
    - `model` and `language`.
    - With `--media`, `--stream` selects the stream. Without `--stream`, it is the first stream that was transcribed.
- `media transcribe`: `media[]`: `media`, `stream`, `model`, `device`, `language`, `timing`, `find`, `words` (how many), `speakers_by` (when speakers were asked for), `warnings`.
- `media details`:
  - `media`, `path`, `size`, `format` (the long name of the container), `bit_rate` and `tags`.
  - `streams[]`: `index`, `kind`, `codec`, `codec_name`, `profile`, `bit_rate`, `pix_fmt`, `bits`, `chroma`, `primaries`, `transfer`, `matrix`, `range`.
  - Also in `streams[]`: `sample_rate`, `channels`, `layout`, `sample_format`, `language`, `tags`.
- `media leave-out`: `left_out`, the spans of the media that are left out after the change.
- `edit ripple-delete-words`: `removed[]`: `start` and `end` (the spans taken out, with touching spans joined), `frames`.
- `subtitles retime`:
  - `track`, `cues`, `heard` (the tracks heard that have sound) and `moved`.
  - `removed`: the cues that would end before zero.
  - `speech`: `offset`, `speed`, `ratio`, `peak`, `rival`, `rival_offset`, `clarity`, `clear`, `needed`.
  - `words`, with `--words`: `media`, `model`, `device`, `language`, `matched`, `unmatched` (cue ids), `speech_errors`, `levels[]`.
  - `correction`: `kind` (`offset`, `speed`, `sections` or `words`), `offset`, `speed`, `ratio`, `sections[]`, `snapped`, `errors`.
  - `warnings`.
  - The errors are `median`, `p90` and `within` (the share within 0.5 s), in seconds, over the matched cues.
- `subtitles time-words`:
  - `track`, `cues` (how many were timed) and `words`.
  - `matched`: the words heard where they are.
  - `unmatched`: the ids of the cues with no word heard. vidcut spreads their words.
  - `heard`, `hear`, `media`, `model`, `device`, `language` and `warnings`.
- `media list`, `media get`:
  - `missing`: the file is gone.
  - `proxy`: `file`, `path` (in the proxy folder of this computer), `width`, `height`, `codec`.
    - `stale`: the proxy was made from another version of the file, or with another `color_space`.
    - `missing`: the file of the proxy is gone.
  - `transcripts[]`: `stream`, `key`, `label`, `model`, `language`, `timing`, `words` (a count), `made` (seconds since 1970), `main`.
    - `main`: the project takes its words from this transcript.
  - `words[]`: for each stream, the main transcript that the project selected, and the words that it changed (`media.words.set`).
  - `left_out`: the source spans that the Transcription window leaves out.
  - `subtitles`, for a subtitle file: `format`, `encoding`, `cues`, `language`, `first` (the text of its first cue), `warnings`.
- `subtitles import`:
  - `track` and `cues`.
  - `placed`: `at`, `timecode` or `start`.
    - `timecode`: the times of the file, read as the timecode of the timeline.
    - `start`: the times of the file, counted from the start of the timeline.
  - `format`, `encoding`, `time_base`, `time_base_why`, `moved_by`, `moved_by_frames` and `warnings`.
- `media add-folder`: `bin` (the bin of the folder), `media` (in the order of the folder), `skipped[]`: `path`, `why`.
- `media relink`: `relinked`, `different` (with `why`), `not_found`.
- `player state`:
  - For each viewer (`timeline`, `source`): `position`, `playing`, `ready`, `hwdec` (the GPU decoder, or `no`).
  - `late` and `skipped`: frames while playing.
  - `audio_device` (null: the default) and `audio_error`.
  - `loop`: Loop playback is on.
  - `two_up`: the frames of the trim two-up.
