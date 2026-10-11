# AGENTS.md

Guide for AI agents (and humans) working on this repository. Keep it current: when behavior, commands or conventions change, update this file in the same change.

## What vidcut is

A relatively simple video cutting tool modeled on DaVinci Resolve's **Cut page**:
- media pool, source/timeline viewer, dual timeline, ripple editing
- export with presets, GPU encoding, audio-only, range and per-clip export, as jobs with progress
- timeline interchange with other editors through OpenTimelineIO (`.otio`)
- automatic tools: Ripple Delete Silence, scene and black detection, the Boring Detector, loudness normalization, storyboards
- an MCP server, so AI agents use every command as a tool
- no effects, transitions, titles or color

It is written in Rust:
- **GUI:** egui/eframe with the glow (OpenGL) backend.
- **Media:** the FFmpeg libraries in-process via `ffmpeg-next`. Never shell out to the `ffmpeg` CLI.
- **Preview:** libmpv, loaded at runtime.

**Everything must be scriptable:** each GUI action is a serializable Request, also reachable from the CLI and from a JSON-RPC socket on the running GUI.

## Ground rules

1. **The Cut page is the spec.** Default behavior is whatever Resolve's Cut page does; see [docs/cut-page-spec.md](docs/cut-page-spec.md) (with manual page numbers).
   - Deviate only when you have a clearly better idea, and record it in [docs/deviations.md](docs/deviations.md) with Resolve's behavior and page, our behavior, and the justification.
   - When the manual is silent or contradictory, add a decision to §12 of the spec.
   - The manual is `DaVinci_Resolve_21.1_Reference_Manual.pdf` in `$VIDCUT_TEST_DATA`. Read pages with `pdftotext -f N -l N <pdf> -`; the Cut page is pp. 635–790. Never paste manual text into the repo; describe behavior in your own words.
2. **One mutation path.**
   - Every feature is a `Request` handled by the core Session, so the GUI, CLI, IPC and (later) MCP have identical capabilities.
   - GUI gestures emit one Request when they complete.
   - Never mutate project state from GUI code directly.
3. **No media in git.**
   - Test media, generated fixtures and the manual live in `$VIDCUT_TEST_DATA` (default `~/Downloads/vidcut-tmp`).
   - Real-world clips are listed in `tests/media-manifest.toml` (URL, license, sha256) and fetched by `scripts/fetch-test-media.sh`.
4. **No ffmpeg/ffprobe subprocesses.** Use `vidcut-av`, the FFmpeg libraries. libmpv is used only for interactive preview.
5. **License:** MIT. Don't add dependencies with licenses incompatible with MIT distribution of our source.

## Workspace layout

| Crate | Purpose | Depends on |
|---|---|---|
| `crates/vidcut-core` | Project model, time and timecode math, edit operations (`ops.rs`; tracks in `ops/tracks.rs`, clips following Track 1 in `ops/follow.rs`), Request/Response protocol, Editor (undo/rev), project store, render plans (`render.rs`: top-wins video, mixed audio lanes), preview EDLs (`preview.rs`), OpenTimelineIO reading and writing (`otio.rs`), what the automatic tools decide from measurements (`analysis.rs`), application preferences (`preferences.rs`) | — (pure Rust; fast to build and test) |
| `crates/vidcut-av` | FFmpeg layer: probe, exact-seek decode, encode/mux, export engine (with loudness normalization; the next piece's file opens ahead in `export/ahead.rs`), thumbnails (and their JPEG cache form), waveforms, still frames, measurements for the automatic tools (`analysis.rs`), encoder detection, test-fixture generator | ffmpeg-next |
| `crates/vidcut-mpv` | libmpv via `dlopen` (`libloading`): client API and render API (OpenGL and software) | libmpv.so.2 at runtime only |
| `crates/vidcut` | The `vidcut` binary: Session/host, IPC, CLI (clap), GUI (eframe), later MCP | all of the above |

Inside `crates/vidcut`:

| Module | What |
|---|---|
| `cli.rs`, `main.rs`, `output.rs` | clap command line → one `Request`; human/JSON output; routing |
| `mcp.rs` | `vidcut mcp`: the MCP server (tools generated from the request schema) |
| `storyboard.rs` | `storyboard.render`: a labeled PNG grid of the program's shots |
| `env.rs` | `AvEnv`: probing and the measurements behind the automatic tools, cached |
| `session.rs` | runs requests headless against a project file |
| `preferences.rs`, `thumbcache.rs` | `~/.config/vidcut/preferences.toml` and the pool thumbnails kept in `~/.cache/vidcut/thumbs` |
| `live/` | the GUI's core thread: open project, players, viewer state, waits, command log, export jobs (`jobs.rs`); publishes `Snapshot`s |
| `ipc/` | JSON-RPC over a Unix socket, the instance registry, CLI routing |
| `gui/` | `app.rs`: eframe and GL glue; `workspace.rs`: all panels and UI state (testable without GL); `timeline/`: geometry, gestures, drawing; `export.rs`: Quick Export, the Export dialog, jobs and OTIO windows; `mixer.rs`: the Mixer and meters; `preferences.rs`: File > Preferences; `timeline/tools.rs`: Ripple Delete Silence, the Boring Detector, Dynamic Trim; `pool.rs` (with bins), `viewer.rs`, `video.rs`, `stills.rs` |
| `export.rs` | presets (built-in and user), outputs, export plans and `.otio` exports as the session runs them |

Docs:
- `docs/cut-page-spec.md`: the behavioral spec.
- `docs/deviations.md`: intentional differences from Resolve.
- `docs/roadmap.md`: the use cases vidcut is for, the to-do list (interchange with Resolve), and the next milestones.
- `docs/decisions/`: short architecture decision records (preview, export, GUI and live control, agents: MCP and measurements).

## Build and run

```bash
cargo build --workspace
cargo run -p vidcut            # GUI (no subcommand)
cargo run -p vidcut -- --help  # CLI
```

**System packages** (Ubuntu 26.04):

```bash
sudo apt install libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libswscale-dev libswresample-dev libavdevice-dev libclang-dev libmpv2 rustup
```

**Toolchain notes:**
- **Rust comes from rustup.** `rust-toolchain.toml` pins the version (with rustfmt and clippy), and rustup installs it on the first cargo command in the checkout, whatever rustc the distro ships. `rust-version` in Cargo.toml matches it, so the MSRV-aware resolver picks crates that build with it.
- **Upgrading Rust** is a change of its own: raise `channel` in `rust-toolchain.toml` and `rust-version` together, then fix what the new clippy finds (`scripts/check.sh full`).
- ffmpeg-next is 9.x. Its version numbers follow FFmpeg's, but it detects the installed FFmpeg and compiles only what that version has, so it builds against the system FFmpeg 8.0.
- Dependencies build with `opt-level = 2` even in dev, while our crates stay unoptimized.

## Quality gates

These are mandatory.

| When | Command | Runs |
|---|---|---|
| Before **every commit** (enforced by `.githooks/pre-commit`) | `scripts/check.sh` | `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, unit tests (`--lib --bins`) |
| After a **bigger change**, and before a milestone is called done | `scripts/check.sh full` | the above plus all integration/e2e tests and doc tests |
| Also, when encoders, hwdec or real-media paths change | `scripts/check.sh full --features vidcut-av/gpu-tests` | GPU encode tests (NVENC/VAAPI) |
| Also, when the GUI, live session, IPC or preview change | `scripts/check.sh full --features vidcut/gui-tests` | the real GUI driven through the CLI (needs a display; opens a window briefly) |

**Enable the hook once per clone:** `git config core.hooksPath .githooks`.

**A bigger change** is anything touching:
- time mapping, decode/seek, the export engine
- EDL/preview, the Request/IPC protocol
- the project file format or store, or edit-operation semantics
- more than one crate

**Rules:**
- **Never commit with failing checks.** Don't skip, ignore or delete tests, or add `#[allow(clippy::...)]`, to get green, unless there's a comment justifying it.
- **Unit tests** (`#[cfg(test)]` in `src/`) must not need media files, a GPU, a display or the network, so the fast gate stays fast and deterministic.
- **Integration tests** (`tests/`) may generate fixtures or use downloaded media. They skip with a clear message when something isn't available.

## Testing conventions

- **Fixtures:** generated by `vidcut-av` into `$VIDCUT_TEST_DATA/fixtures/` and cached by spec hash.
  - The video is a **barcode**: the frame index encoded as luma blocks, which survives lossy codecs, so frame-exactness can be asserted after any decode or encode.
  - The audio carries a 1 kHz beep at the start of every second, for measuring A/V offset.
  - Stills and image sequences: `fixture::ensure_images` writes barcode pictures as PNG or JPEG files, one or numbered (`ImageSpec::still`, `ImageSpec::sequence`).
  - Damaged files: `fixture::spoil` copies one with a packet left out or zeroed.
- **Real-world clips:** `scripts/fetch-test-media.sh` downloads the clips in `tests/media-manifest.toml` to `$VIDCUT_TEST_DATA/media/<name>/<file>` and checks their sha256. They cover H.264 1080p30 and 2160p60 (two audio streams each, stereo MP3 and 5.1 AC-3), a 24000/1001 QuickTime file with 44.1 kHz audio, VP8/Vorbis 5.1 WebM with an unknown sample aspect ratio, and a short HEVC clip.
- **Edit-operation tests** cite the spec section or manual page they check.
- **Golden tests** (insta snapshots in `src/snapshots/`) cover protocol output that agents parse.
  - After an intended change, regenerate with `INSTA_UPDATE=always cargo test`.
  - Review the diff before committing.
- **Property tests** (proptest) run random edit sequences.
  - Failing seeds land in `proptest-regressions/`. Commit them, so they replay forever.
  - For a deeper run: `PROPTEST_CASES=5000 cargo test -p vidcut-core prop_tests`.
- **GUI input tests** (`crates/vidcut/tests/gui_input.rs`, `gui_export.rs`, `gui_tracks.rs`, `gui_tools.rs`, `gui_import.rs`) drive the real panels and windows with egui_kittest (clicks, drags, keys; no window or renderer) against a live session without players. Each gesture must change the project with exactly one request. Timeline gesture logic itself is unit-tested in `gui/timeline/gesture.rs` from pointer positions.
  - File dialogs are replaced with `Workspace::set_dialogs`.
  - A new window or modal is measured in its first frame and placed in the next: step once more after it appears before clicking in it.
  - Windows that ask the session (presets, plans, imports) answer on helper threads, so wait with a loop of `harness.step()`, not a fixed number of steps.
  - egui widgets such as sliders only see a drag that spans several frames: step between press, moves and release (`drag_slowly` in `gui_tracks.rs`). Timeline gestures work either way.
  - A freshly opened menu ignores input in its first frame: step twice before clicking an item (`menu_item` in `gui_tools.rs`).
- **MCP** (`crates/vidcut/tests/mcp.rs`, the M7 acceptance) runs `vidcut mcp` as a child process and talks JSON-RPC to it, headless and against a live session serving IPC.
- **Measurement fixtures:** `Picture::Shots` (a new shot every N frames, every fourth one black) and `Sound::Tone` (a steady 1 kHz sine at −20 dBFS, −20 LUFS in stereo) have known answers for scene, black and loudness tests. In core tests, `FakeEnv` takes made-up levels and pictures (`test_support::levels`, `pictures`).
- **Export jobs** (`crates/vidcut/tests/jobs.rs`) run the live session without players: foreground, background, cancel and wait.
- **Live session tests** (`crates/vidcut/tests/live.rs`) run the GUI's core thread without a window: a harness thread renders both players with mpv's software renderer and answers screenshots, and requests go through `Handle::call` like IPC. Prefer them over GUI tests; they need libmpv but no display.
- **Frame checks** compare barcodes: screenshot the viewer, decode the PNG with `vidcut_av::decode`, and compare with `vidcut frame` (export's renderer).
- **Test outputs** go to temp dirs, never into the repo. Tests that open projects set `XDG_STATE_HOME` to a temp dir so undo history and recent projects stay out of the user's state, tests that save presets or preferences set `XDG_CONFIG_HOME`, and tests that check thumbnails set `XDG_CACHE_HOME`.

## Driving vidcut as an agent

*Status: the CLI edits and exports headless (M1, M2), controls a running GUI (M3), exports in the background and exchanges OTIO timelines (M5), edits several tracks with a mixer and bins (M6), serves MCP with automatic tools and storyboards (M7), handles rotated, interlaced and HDR media (M8), and stills and image sequences (M9).*

**MCP** (docs/decisions/agents.md, deviation X10): `vidcut mcp [-p FILE]` serves the Model Context Protocol on stdin/stdout. Every command is a tool (`edit.split` is `edit_split`, with its JSON Schema), plus `use_project` to pick or create the project. Calls run like the CLI's: inside the GUI that has the project open, or headless. `frame_render`, `gui_screenshot` and `storyboard_render` return images. To add it to Claude Code: `claude mcp add vidcut -- vidcut mcp -p /abs/talk.vidcut`; other clients take `{"command": "vidcut", "args": ["mcp", "-p", "/abs/talk.vidcut"]}`.

**Discovering commands:**
- Every CLI subcommand is one JSON-RPC method on one `Request`.
- `vidcut schema` prints the whole protocol as JSON Schema; `vidcut schema edit.trim` prints one method's params.
- `vidcut --help` and `vidcut <cmd> --help` show examples.

**Basic use:**

```bash
vidcut new talk.vidcut                      # format comes from the first video added
export VIDCUT_PROJECT=talk.vidcut           # or pass -p FILE every time
vidcut media add cam.mp4 screen.mkv --json  # -> media ids m1, m2
vidcut edit append m1 --range 10s..2m
vidcut edit split --at 1m,1m30s
vidcut edit delete V1@1m10s                 # the clip under 1m10s; ripples Track 1
vidcut timeline --json                      # clips and gaps with frames and timecodes
vidcut undo                                 # works across separate runs
```

**Times:**
- In strings, every time needs a unit: `450f`, `15s`, `1m30s`, `250ms`, `00:01:00:00` (non-drop-frame timecode), `HH:MM:SS.mmm`, `start`, `end`, `in`, `out`.
- In JSON params, a bare integer is a frame number.
- Ranges are half-open: `A..B` excludes B; `A..+D` is D long.

**Clips:** refer to them by id (`c17`) or by position (`V1@<time>`, `V2@1m`).

**Tracks (M6):**
- `vidcut track` lists them. `track add video|audio [--count N]`, `track remove V3 [--force]` (`--force` also deletes its clips), and `track set A1 --volume -6 --name Music` with `--lock`/`--unlock`, `--mute`/`--unmute`, `--solo`/`--unsolo`, `--video false`.
- Track ids are positions: V1, V2, … from the bottom and A1, A2, … (spec D27). Removing a track renumbers the later ones, so list them again afterwards.
- Edit functions take `--track V2` (the target track) and `--audio-only`/`--video-only`. Naming the next new track (V3 when V2 is the last) creates it. Media without video go to A1 by default.
- `vidcut edit place-on-top m4 --range 5s..+3s` puts B-roll on the first track above everything at the playhead (or `--at`, or the timeline marks); the result names the clip and the track. `vidcut edit move c7 --to 2m --track V2` moves clips between tracks.
- Track 1 ripples. Clips on other tracks follow the Track 1 picture under their first frame (spec D21); other tracks ripple only where Track 1 is empty (D22). Only Track 1 pushes clips: inserts elsewhere overwrite (D23).
- Export, `frame` and the preview show the highest visible clip and mix every audible track with its fader. Mute and solo apply to the export too (p. 650).

**Stills and image sequences (M9, spec D38, D39):**
- `vidcut media add photo.jpg` adds a still. Edited without `--range` it lasts the project's still duration (5 s; `vidcut set --still-duration 3s`); a range or trim makes it up to 17 h 40 min long.
- `vidcut media add renders/frame_000001.png --sequence [--fps 60]` adds the numbered files in a row around it as one clip (`frame_[000001-010560].png`), at the project's rate unless `--fps` is given. Without `--sequence` a numbered image is a still.
- `media get` reports `still: true`, or `sequence: {pattern, first, count}`.

**Bins (M6):** `vidcut bins` lists them. `bins add "Interviews/Day 1"` (parents too), `bins rename A B`, `bins remove A` (its media move up a level, deviation X6), `media add FILE --bin A`, `media move m1 m2 --bin A` (`""` is the top level). `media list` shows each item's bin.

**JSON output:**
- `--json` prints `{"rev", "changes": {"created", "modified", "deleted"}, "result", "via"}`.
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
- `--if-rev N` fails with `CONFLICT` if someone else changed the project after you read revision `N`. Every request on the project checks it: edits, exports, frames, measurements, queries, and live ones such as seeks and screenshots. An export encodes the project as it was when it started, whatever edits follow.
- `vidcut apply edits.jsonl` (or `-` for stdin) applies many requests atomically, as one undo step. The format is one `{"method", "params"}` object per line.
- Parallel `vidcut` processes on one project are serialized by a lock and never corrupt it.

**Raw requests:** `vidcut rpc edit.split '{"at": ["1m", 2700]}'` sends one exactly as written.

**Export and stills:**
- `vidcut export out.mp4` renders the timeline: its In–Out range when both marks are set, else everything.
  - The extension picks the container, and also the preset unless `--preset` is given.
  - Extra options: `--preset youtube-1080p`, `--encoder nvenc|vaapi|cpu|auto`, `--set video.quality=20`, `--range 1m..2m`, `--per-clip`, `--audio-only`, `--overwrite`.
  - With `--dry-run` it prints the plan and the chosen encoder without encoding.
  - Ctrl-C cancels cleanly (exit 9, no partial file).
  - With a GUI running, `--background` returns a job id at once; follow it with `vidcut jobs` (below).
- `vidcut presets` lists the presets (`--json` includes their settings); `vidcut presets NAME` shows one.
  - `vidcut presets save web --from youtube-1080p --set video.quality=22 [--quick-export]` saves a user preset to `~/.config/vidcut/presets/web.toml`; `--settings JSON|@FILE` gives full settings instead. `vidcut presets delete web` removes it.
- `vidcut encoders` shows which GPU encoders work here (cached; `--refresh` re-tests).
- `vidcut preferences` shows the application preferences (`~/.config/vidcut/preferences.toml`) and what the thumbnail cache holds; `vidcut preferences set thumbnails.cache_limit_mb=256` changes them (`preferences.set` takes `{"thumbnails": {"disk_cache": false}}`; turning the cache off empties it).
- `vidcut frame --at 1m -o f.png [--width 640]` renders what the timeline shows at a frame. `--media m1` renders a media frame instead. Use it to look at cuts.

**Automatic tools (M7, spec D29–D32):**
- `vidcut edit ripple-delete-silence c1 [c2…] [--min 0.5s --threshold -40 --pre-head 0.15s --post-tail 0.15s]` (the defaults shown) cuts silent parts out of clips and closes the gaps, as one undo step. `--dry-run` lists the parts.
- `vidcut edit detect-scene-cuts c1` splits a clip where its shots change (`--threshold`, scene score 0–100, default 10).
- `vidcut analyze silence|scenes|black [--media m1 | --clips c1,c2]` and `vidcut analyze loudness [--range a..b]` measure without changing anything. Media positions are source frames; clip positions are timeline frames.
- `vidcut boring [--boring 30s --jump 10f]` lists clips too long and too short (the Boring Detector).
- `vidcut storyboard -o board.png [--every 5s --columns 4 --width 320]` draws a picture of every shot with timecode and clip id: the quickest way to look at a whole edit.
- `vidcut export out.mp4 --set audio.loudness=-14` normalizes the mix to −14 LUFS (one gain, true peak ≤ −1 dBTP); the result reports the gain.
- `vidcut export out.mp4 --set video.codec=hevc --set video.hdr=pq` (or `hlg`) exports HDR in 10 bits (HEVC, AV1 or VP9). Without it, HDR sources are tone-mapped to SDR. `media get` reports each clip's `hdr`, `rotation` and interlaced `fields` (M8).
- `vidcut edit slide c3 --by 5f` moves a clip keeping its media; its neighbors give and take.
- Measuring decodes a file once; the results are cached (`~/.cache/vidcut/analysis/`), so trying other settings is quick.

**Timeline interchange (OpenTimelineIO, deviation X9):**
- `vidcut export cut.otio` writes the whole timeline (or `--range`) for other editors: each video track, its clips' audio (J/L cuts included) as an audio track, each audio track, markers, and vidcut extras (marks, clip gain, track settings) in `metadata.vidcut`. Image sequences go as `ImageSequenceReference`s whose available range counts the pictures at the sequence rate, and come back as sequences.
- `vidcut import cut.otio` adds the file's media to the pool and the file's tracks after the last clip: video tracks onto V1, V2, … with the audio that belongs to their clips, other audio tracks onto A1, A2, …. `--replace` replaces the timeline instead. An empty timeline takes the file's frame rate, start timecode and track settings.
  - Media that moved: `--search /mnt/footage` looks for them by file name, subfolders too. Missing media fail the import (exit 7) with the list of files.
  - What can't be imported (transitions, nested stacks, generators, clips overlapping others on their track) is listed in `warnings`.
- Raw: `timeline.import` also takes the document inline as `otio`.

**Live control of a running GUI** ([docs/decisions/gui.md](docs/decisions/gui.md)):
- **Routing:** when a GUI has the project open (`vidcut -p FILE` starts one), every project command runs inside it. You and the human then edit one live session: the GUI updates at once, and `via` in the JSON says `gui:<pid>`.
  - `--headless` forces a separate run; `--instance PID|SOCKET` (or `VIDCUT_SOCKET`) picks a GUI.
  - `vidcut instances` lists running GUIs.
- **Live-only commands:**

| Command | Request | What |
|---|---|---|
| `vidcut state` | `player.state` | viewer mode, both playheads, the source clip, selection |
| `vidcut seek 10s [--viewer timeline\|source\|tape]` | `player.seek` | returns once the frame is on screen (`CONFLICT` if someone moved the viewer first); `+10f`/`-2s` are relative |
| `vidcut step [-]N`, `play [--speed -2]`, `pause` | `player.*` | negative speeds play backwards |
| `vidcut viewer source\|tape\|timeline`, `load m1 [--at 5s]` | `viewer.*` | what the viewer shows |
| `vidcut select --clips c1,c2 --media m3` | `selection.set` | without flags: `selection.get` |
| `vidcut screenshot -o v.png [--window]` | `gui.screenshot` | the viewer picture as shown (works while minimized), or the whole window |
| `vidcut log [--since N]` | `gui.log` | what everyone (the human too) did, as requests |
| `vidcut window --size 1600x1000`, `open FILE [--new]`, `quit` | `gui.*`, `project.open` | window and project |
| `vidcut jobs`, `jobs get\|wait\|cancel J [--timeout S]` | `jobs.*` | export jobs, the GUI's own included; `wait` fails like the job did |
| `vidcut pool --bin A --sort name --descending` | `pool.set` | the bin the Media Pool shows and its order; Source Tape follows it |
| `vidcut mute [on\|off]` | `player.mute` | the viewer's monitoring mute (not the export) |
| `vidcut show quick-export\|export\|jobs\|log\|mixer` | `gui.show` | opens that window for the person at the GUI |

- **Looking at the result:** `vidcut seek 1m && vidcut screenshot -o shot.png` shows exactly what `vidcut frame --at 1m -o f.png` renders. Both are frame-exact.
- **Exporting alongside the human:** `vidcut export out.mp4 --background` returns at once; the export shows in the GUI's Jobs window, and `vidcut jobs wait j1` blocks until it's done. An export the human starts from Quick Export is a job too, so `vidcut jobs` sees it.
- **Source marks and clipboard work headless too:**
  - `vidcut media marks m1 --in 5s --out 10s`: edits without `--range` then use these marks.
  - `vidcut edit copy c3 --json`, then `vidcut edit paste --clips @clips.json --at 1m`.
- **Paths:** the CLI makes paths absolute before routing, those in `apply` batches too. In raw `rpc` requests, use absolute paths.

## Tools

**Fixtures:** generate one (cached, prints its path):

```bash
cargo run --release -p vidcut-av --example make_fixture -- hevc10 mkv 3840x2160 60 600
```

**GPU encoders:** `cargo run -p vidcut-av --example encoders` test-encodes every NVENC/VAAPI option, uncached.

**FFmpeg logging:** quiet by default (fatal only). Set `VIDCUT_AV_LOG=error` (or `warning`, `debug`) to see FFmpeg's messages when debugging media problems.

**Media inspection** (examples in `crates/vidcut-av/examples/`):
- `read_barcode FILE...`: the barcode index in images or video (compare screenshots with `vidcut frame`).
- `packets FILE [FROM TO]`: video packets in decode order, with pts, dts and keyframe flags (open-GOP questions).
- `filmstrip FILE OUTDIR [N]`: pool thumbnails, timed.

**Preview debugging:**
- `VIDCUT_DEBUG_PLAYER=1` traces every player command, mpv event, render and timeline gesture to stderr.
- `VIDCUT_MPV_OPTIONS="hwdec=no,ao=null"` overrides mpv options.
- `VIDCUT_MPV_LOG_DIR=DIR` makes each GUI player write mpv's own log, at debug level, to `DIR/<pid>-source.log` and `DIR/<pid>-timeline.log`. `VIDCUT_MPV_OPTIONS=msg-level=all=trace` adds more.
  - FFmpeg's messages are all in the timeline's log: both players' and those of vidcut's own FFmpeg work in the GUI process (thumbnails, waveforms, exports). FFmpeg has one log callback per process, and the first mpv instance (the timeline's) takes it.
  - `tests/gui.rs` sets it and `VIDCUT_DEBUG_PLAYER` for its GUI. A failed run keeps its temp dir, which the failure names: the screenshots, both mpv logs and the GUI's stderr.

**Preview spikes:**
- `cargo test -p vidcut-mpv --test edl_spike -- --nocapture` runs the mpv EDL/seek semantics experiments headless.
- `cargo run -p vidcut --example mpv_gl_spike -- FILE [FILE2] --auto-exit 8` opens a window with mpv embedded in egui and prints hwdec and drop statistics.

## Gotchas

The preview findings and the time-mapping rules are in [docs/decisions/preview.md](docs/decisions/preview.md); the export engine, sampling rule and encoder decisions are in [docs/decisions/export.md](docs/decisions/export.md). Read them before touching preview, seeking, time conversion or export.

- **libmpv needs `LC_NUMERIC=C`.** `Mpv::new` calls `setlocale` before `mpv_create`.
- **mpv's render thread (the GUI's UI thread) may only call render-thread-safe client functions:**
  - async commands and property calls
  - `mpv_observe_property`, `mpv_wakeup`
  - `mpv_wait_event(…, 0)`

  Synchronous `command`/`get_property` there deadlocks against the video output. The mpv update and wakeup callbacks must not call mpv at all.
- **vsync must stay off** (`GlowConfiguration { vsync: false }`). On GNOME/Wayland with NVIDIA, a vsynced swap blocks forever while the window is hidden.
- **Players render in `App::logic`, not `App::ui`.** eframe skips `ui` for hidden windows but always calls `logic`.
- **libmpv waits only 200 ms for the renderer to take a new frame.** Then it keeps the frame as its current picture and finishes the seek anyway, so "a new frame was rendered" doesn't prove a seek is on screen. The session counts a seek as shown only after the renderer presents the player following mpv's restart (docs/decisions/gui.md §4).
- **mpv's EDL drops leading pictures at segment ends.** Encodes that decode a keyframe before the frames shown just ahead of it lose those frames in the preview when a cut ends there. The live session patches such tails (preview.md §9). Test cuts against fixtures with B-frames and real media, not only small fixtures.
- **Only the session thread calls mpv's client API.** Seeks while a file is still loading are rejected by mpv; `Player::seek` reloads instead.
- **File watching:** `notify` reports reads as access events. Ignore them, or a reload re-triggers itself forever.
- **The half-frame bias is applied exactly once.** mpv EDL `start` values are raw source timestamps and carry it, so timeline seeks to frame `j` go to exactly `j/f`.
- **Never send negative absolute seeks.** mpv counts them from the end of the file.
- **MPEG-TS and other index-less containers can't be seeked exactly by mpv.** Preview uses an MKV remux (`vidcut_av::remux`); export reads the original.
- **Set packet durations explicitly when muxing** from delayed encoders (x264 with B-frames). Otherwise MP4 edit lists drop the last frame.
- **Set `pkt_timebase` on every decoder** (`codec_ctx.pkt_timebase = stream.time_base`). Without it, FFmpeg can't adjust timestamps when it skips priming samples, and audio drifts by the codec delay.
- **Color goes through `vidcut_av::color`** (export.md §4): HDR sources are tone-mapped for SDR output, SDR sources sit at 203 nits in HDR output, untagged HD is BT.709. Rotation metadata is applied everywhere (spec D33).
- **`zscale` needs explicit output primaries** when converting to linear light: for untagged frames it otherwise fails with "no path between colorspaces".
- **Interlaced sources are deinterlaced inside `VideoReader`** (`bwdif`, one frame per frame), before frames are picked. The preview adds the same filter per file (preview.md §5); mpv's own `deinterlace` makes one frame per field, which breaks frame-exact seeks.
- **mpv's software renderer aborts on rotated video** (an assertion in `mp_image_crop`). The GUI renders with OpenGL, which rotates on the GPU, so `tests/live.rs` (software rendering) leaves rotated clips to `tests/gui.rs`.
- **GPUs on the dev machine:**
  - NVIDIA on `renderD128`: NVENC works, VAAPI doesn't.
  - AMD on `renderD129`: VAAPI H.264/HEVC.
  - The GL context is on NVIDIA, so preview hwdec is nvdec with zero-copy CUDA frames.
