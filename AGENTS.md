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
2. **The code never names DaVinci Resolve.** Nothing in `crates/`, comments included, mentions Resolve or cites its manual's pages, and neither does anything vidcut shows or prints (GUI text, `--help`, the schema, MCP text, errors). Cite the spec instead (`spec §7`, `decision D41`, `deviation X9`); the spec maps to the manual. The only exception is file-format data other editors need, such as the OTIO metadata keys they read.
3. **One mutation path.**
   - Every feature is a `Request` handled by the core Session, so the GUI, CLI, IPC and MCP have identical capabilities.
   - GUI gestures emit one Request when they complete.
   - Never mutate project state from GUI code directly.
4. **No media in git.**
   - Test media, generated fixtures and the manual live in `$VIDCUT_TEST_DATA` (default `~/Downloads/vidcut-tmp`), the one place for test data. Test outputs go to temp dirs.
   - `scripts/check.sh` (the pre-commit hook and CI) refuses any binary file, whatever its name, so `.gitignore` lists no file types.
   - Real-world clips are listed in `tests/media-manifest.toml` (URL, license, sha256) and fetched by `scripts/fetch-test-media.sh`.
5. **No ffmpeg/ffprobe subprocesses.** Use `vidcut-av`, the FFmpeg libraries. libmpv is used only for interactive preview.
6. **License:** MIT. Don't add dependencies with licenses incompatible with MIT distribution of our source.

## Workspace layout

| Crate | Purpose | Depends on |
|---|---|---|
| `crates/vidcut-core` | Project model, time and timecode math, edit operations (`ops.rs`; tracks in `ops/tracks.rs`, clips following Track 1 in `ops/follow.rs`), Request/Response protocol, Editor (undo/rev), project store, render plans (`render.rs`: top-wins video, mixed audio lanes), preview EDLs (`preview.rs`), OpenTimelineIO reading and writing (`otio.rs`, tests in `otio/tests.rs`), what the automatic tools decide from measurements (`analysis.rs`), application preferences (`preferences.rs`) | — (pure Rust; fast to build and test) |
| `crates/vidcut-av` | FFmpeg layer: probe (start timecodes too), exact-seek decode (on NVDEC too, `cuda.rs` for the GPU pipeline), encode/mux, export engine (with loudness normalization; the next piece's file opens ahead in `export/ahead.rs`), the renderer of paused and two-up pictures that decodes each frame once either way, and the frame reader of timeline tiles (`export/renderer.rs`), thumbnails (and their JPEG cache form), waveforms, still frames, measurements for the automatic tools (`analysis.rs`), encoder detection, test-fixture generator, the player engine (`player/`: a decode thread keeping decoded YUV pictures ahead of the clock and windows for going back, the audio mix into a lock-free ring, reversed backward and time-stretched at other speeds keeping the pitch (`player/stretch.rs`, which marks each block for the clock), the device callback as the clock, outputs through cpal, audio scrubbing in `player/scrub.rs` with `Scrubber` for use without a player) | ffmpeg-next, cpal |
| `crates/vidcut-mpv` | libmpv via `dlopen` (`libloading`): client API and render API (OpenGL and software) | libmpv.so.2 at runtime only |
| `crates/vidcut` | The `vidcut` binary: Session/host, IPC, CLI (clap), GUI (eframe), MCP server | all of the above |

Inside `crates/vidcut`:

| Module | What |
|---|---|
| `cli.rs`, `main.rs`, `output.rs` | the command line, generated from the request schema (method `group.verb` is `vidcut group verb`) → one `Request`; human/JSON output; routing |
| `mcp.rs` | `vidcut mcp`: the MCP server (tools generated from the request schema) |
| `storyboard.rs` | `storyboard.render`: a labeled PNG grid of the program's shots |
| `env.rs` | `AvEnv`: probing and the measurements behind the automatic tools, cached |
| `session.rs` | runs requests headless against a project file |
| `preferences.rs`, `thumbcache.rs` | `~/.config/vidcut/preferences.toml` and the pool thumbnails kept in `~/.cache/vidcut/thumbs` |
| `threads.rs` | the one way the app starts threads: `threads::spawn(name, Work::Job \| Work::Interactive, f)`; media work runs at the lowest CPU priority |
| `live/` | the GUI's core thread: open project, players, viewer state, waits, command log, export jobs (`jobs.rs`); publishes `Snapshot`s |
| `ipc/` | JSON-RPC over a Unix socket, the instance registry, CLI routing |
| `gui/` | `app.rs`: eframe and GL glue; `workspace.rs`: all panels and UI state (testable without GL); `timeline/`: geometry, gestures, drawing; `export.rs`: Quick Export, the Export dialog, jobs and OTIO windows; `mixer.rs`: the Mixer and meters; `preferences.rs`: File > Preferences; `project.rs`: New Project's format and File > Project Settings; `timeline/tools.rs`: Ripple Delete Silence, the Boring Detector, Dynamic Trim; `tiles.rs`: the timeline's filmstrip pictures, each tile's own frame; `folders.rs`: where file dialogs start; `pool.rs` (with bins), `viewer.rs`, `video.rs`, `stills.rs` |
| `export.rs` | presets (built-in and user), outputs, export plans and timeline (`.otio`, bundle) exports as the session runs them |
| `bundle.rs` | OpenTimelineIO bundles: writing `.otioz` (zip) and `.otiod` (folder) with their media, unpacking `.otioz` for import |

Docs:
- `docs/cut-page-spec.md`: the behavioral spec.
- `docs/deviations.md`: intentional differences from Resolve.
- `docs/roadmap.md`: the use cases vidcut is for, and the order of what comes next.
- `docs/todo/`: open work, **one file per item**: add a file for new work, and delete it in the pull request that finishes the work. There is no list to edit, so pull requests don't conflict over it. Keep lists that pull requests edit one item per line, never several items in one line.
- `docs/decisions/`: short architecture decision records (preview, export, GUI and live control, agents: MCP and measurements, the player engine).

## Build and run

```bash
cargo build --workspace
cargo run -p vidcut            # GUI (no subcommand)
cargo run -p vidcut -- --help  # CLI
```

**System packages** (Ubuntu 26.04):

```bash
sudo apt install libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libswscale-dev libswresample-dev libavdevice-dev libasound2-dev libclang-dev libmpv2 rustup
```

**Supported FFmpeg:** 8.0, the one in the latest Ubuntu LTS (26.04), up to the newest FFmpeg release (9.0.2, `scripts/ffmpeg/release.env`). Both are tested (`scripts/ffmpeg-check.sh`, the weekly FFmpeg workflow); nothing older needs to work, so add no code for FFmpeg before 8.0. Where FFmpeg versions in that range behave differently, handle both by what the file or stream reports, not by version numbers.

**Toolchain notes:**
- **Rust comes from rustup.** `rust-toolchain.toml` pins the version (with rustfmt and clippy), and rustup installs it on the first cargo command in the checkout, whatever rustc the distro ships. `rust-version` in Cargo.toml matches it, so the MSRV-aware resolver picks crates that build with it.
- **Upgrading Rust** is a change of its own: raise `channel` in `rust-toolchain.toml` and `rust-version` together, then fix what the new clippy finds (`scripts/check.sh full`).
- ffmpeg-next is 9.x. Its version numbers follow FFmpeg's, but it detects the installed FFmpeg and compiles only what that version has, so it builds against FFmpeg 8.0 and 9.0 alike.
- Dependencies build with `opt-level = 2` even in dev, while our crates stay unoptimized.

## Quality gates

These are mandatory.

| When | Command | Runs |
|---|---|---|
| Before **every commit** (enforced by `.githooks/pre-commit`) | `scripts/check.sh` | no binary files in git, `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, unit tests (`--lib --bins`) |
| After a **bigger change**, and before a milestone is called done | `scripts/check.sh full` | the above plus all integration/e2e tests and doc tests |
| Also, when encoders, hwdec or real-media paths change | `scripts/check.sh full --features vidcut-av/gpu-tests` | GPU encode tests (NVENC/VAAPI) |
| Also, when the GUI, live session, IPC or preview change | `scripts/check.sh full --features vidcut/gui-tests` | the real GUI driven through the CLI (needs a display; opens a window briefly) |
| Also, when the player's audio output changes | `scripts/check.sh full --features vidcut-av/audio-tests` | the player engine on the system's audio device (plays silence) |
| Also, when FFmpeg-facing code changes (probe, decode, encode, filters, ffmpeg-next upgrades) | `scripts/ffmpeg-check.sh` | `cargo build` and `scripts/check.sh full` in containers, against Ubuntu 26.04's FFmpeg 8.0 and the newest FFmpeg release (Tools, below) |

**Enable the hook once per clone:** `git config core.hooksPath .githooks`.

**CI** (`.github/workflows/check.yml`) runs the fast gate on pull requests and on pushes to master, on `ubuntu-26.04` with Ubuntu's FFmpeg 8.0: older images ship FFmpeg 6. The compiled dependencies are cached (`Swatinem/rust-cache`), so a run builds only vidcut's own crates and the dependencies `Cargo.lock` changed; after a toolchain upgrade it builds everything once. The full gate, GPU and GUI tests run only locally. Dependabot opens weekly update pull requests.

**The FFmpeg workflow** (`.github/workflows/ffmpeg.yml`) runs weekly (Mondays) and on demand (Actions > ffmpeg > Run workflow), never on pull requests or pushes. It builds the newest FFmpeg release and FFmpeg's master from source with `scripts/ffmpeg/build-ffmpeg.sh` (about ten minutes on a runner, cached), then runs `cargo build` and the fast gate against each. A master failure doesn't fail the run.

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
  - `.timecode("10:00:00:00")` records a camera start timecode (a `tmcd` track in MP4/MOV), for timeline interchange tests.
  - Stills and image sequences: `fixture::ensure_images` writes barcode pictures as PNG or JPEG files, one or numbered (`ImageSpec::still`, `ImageSpec::sequence`).
  - Damaged files: `fixture::spoil` copies one with a packet left out or zeroed.
- **Real-world clips:** `scripts/fetch-test-media.sh` downloads the clips in `tests/media-manifest.toml` to `$VIDCUT_TEST_DATA/media/<name>/<file>` and checks their sha256. They cover H.264 1080p30 and 2160p60 (two audio streams each, stereo MP3 and 5.1 AC-3), a 24000/1001 QuickTime file with 44.1 kHz audio, VP8/Vorbis 5.1 WebM with an unknown sample aspect ratio, and a short HEVC clip.
- **Edit-operation tests** cite the spec section or decision they check.
- **Golden tests** (insta snapshots in `src/snapshots/`) cover protocol output that agents parse.
  - After an intended change, regenerate with `INSTA_UPDATE=always cargo test`.
  - Review the diff before committing.
- **Property tests** (proptest) run random edit sequences.
  - Failing seeds land in `proptest-regressions/`. Commit them, so they replay forever.
  - For a deeper run: `PROPTEST_CASES=5000 cargo test -p vidcut-core prop_tests`.
- **GUI input tests** (`crates/vidcut/tests/gui_input.rs`, `gui_export.rs`, `gui_tracks.rs`, `gui_tools.rs`, `gui_import.rs`, `gui_tiles.rs`, `gui_project.rs`) drive the real panels and windows with egui_kittest (clicks, drags, keys; no window or renderer) against a live session without players. Each gesture must change the project with exactly one request. Timeline gesture logic itself is unit-tested in `gui/timeline/gesture.rs` from pointer positions.
  - File dialogs are replaced with `Workspace::set_dialogs`.
  - A new window or modal is measured in its first frame and placed in the next: step once more after it appears before clicking in it.
  - Windows that ask the session (presets, plans, imports) answer on helper threads, so wait with a loop of `harness.step()`, not a fixed number of steps.
  - `Work::Job` threads (timeline tiles, thumbnails) run in the idle class, which a busy machine starves for seconds: wait for the state the test checks (`Workspace::timeline_tiles_pending`, the files on disk) with a generous deadline, never for a quiet spell.
  - egui widgets such as sliders only see a drag that spans several frames: step between press, moves and release (`drag_slowly` in `gui_tracks.rs`). Timeline gestures work either way.
  - A freshly opened menu ignores input in its first frame: step twice before clicking an item (`menu_item` in `gui_tools.rs`).
- **MCP** (`crates/vidcut/tests/mcp.rs`) runs `vidcut mcp` as a child process and talks JSON-RPC to it, headless and against a live session serving IPC.
- **Color fixtures:** `Picture::Colors` paints four colored stripes (`Picture::COLORS`, in Y′CbCr) for color conversion tests.
- **Measurement fixtures:** `Picture::Shots` (a new shot every N frames, every fourth one black) and `Sound::Tone` (a steady 1 kHz sine at −20 dBFS, −20 LUFS in stereo) have known answers for scene, black and loudness tests. In core tests, `FakeEnv` takes made-up levels and pictures (`test_support::levels`, `pictures`).
- **Export jobs** (`crates/vidcut/tests/jobs.rs`) run the live session without players: foreground, background, cancel and wait.
- **Live session tests** (`crates/vidcut/tests/live.rs`) run the GUI's core thread without a window: a harness thread renders both players with mpv's software renderer and answers screenshots, and requests go through `Handle::call` like IPC. Prefer them over GUI tests; they need libmpv but no display.
- **Frame checks** compare barcodes: screenshot the viewer, decode the PNG with `vidcut_av::decode`, and compare with `vidcut frame render` (export's renderer).
- **Player engine tests** (`crates/vidcut-av/tests/player*.rs`, shared helpers in `tests/player_support/`) run headless and compare each picture's barcode with `frame render`'s (`export::still`). Pictures are decoded YUV at the source's size: `read_barcode` reads any layout, and `Picture::to_rgba` gives what `frame render` does.
  - The sound is the clock, so a test drives it with an output: `ManualOutput` pulls when the test says (`pull_full`: a buffer as if just heard, so the clock stands at its end until the next), `PacedOutput` pulls in real time with a set latency and can record what it pulled and when it would be heard (A/V offset from the beeps).
  - Measure picture against sound with `ClockTrace` (the clock read at exact instants through `Player::frame_at`), not by when a polling loop saw a frame: a loaded machine runs the test thread late, not the clock.
  - Audio scrubbing is on by default, so a seek before play queues that frame's sound ahead of the play's: tests that count samples from play turn it off (`Config { audio_scrubbing: false, .. }`). `player_sound.rs` checks reversed sound and scrubbing against the beeps.
  - Decode counts (`vidcut_av::decode::frames_decoded`, process-wide) stay in `player_decodes.rs`, one test in its own binary. Work out expected pictures before counting: `frame render` decodes too.
  - `player_media.rs` prints seek, step and play numbers on the real 1080p30 and 2160p60 clips (`--nocapture`); `player_device.rs` (`audio-tests`) plays silence on the system's device.
- **Test outputs** go to temp dirs, never into the repo. Make test temp dirs with `tempfile` (`tempfile::Builder::new().prefix("vidcut-…").tempdir()`), which removes them when they drop, also when the test fails. Tests that open projects set `XDG_STATE_HOME` to a temp dir so undo history, recent projects and the folders file dialogs remember stay out of the user's state, tests that save presets or preferences set `XDG_CONFIG_HOME`, and tests that check thumbnails set `XDG_CACHE_HOME`.

## Driving vidcut as an agent

**MCP** (docs/decisions/agents.md, deviation X10): `vidcut mcp [-p FILE]` serves the Model Context Protocol on stdin/stdout. Every command is a tool (`edit.split` is `edit_split`, with its JSON Schema), plus `use_project` to pick or create the project. Calls run like the CLI's: inside the GUI that has the project open, or headless. `frame_render`, `gui_screenshot` and `storyboard_render` return images. Cancelling a call stops a headless export; routed into the GUI, it also stops a frame, a storyboard, an analysis and the loudness measure before a foreground export. A background export job stops only with `jobs.cancel`. To add it to Claude Code: `claude mcp add vidcut -- vidcut mcp -p /abs/talk.vidcut`; other clients take `{"command": "vidcut", "args": ["mcp", "-p", "/abs/talk.vidcut"]}`.

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
- `vidcut proxy generate [m1 m2…] [--width 960]` writes an 8-bit 4:2:0 proxy for each video or image sequence (default longest side 960; smaller pictures stay their size), with a keyframe every 60 frames and every frame coded in the order it shows. With NVENC, sources the GPU shows as the CPU would (a video file; turned pictures, BT.601, full range and HDR go through libplacebo on Vulkan) decode on NVDEC and scale on the GPU, the frames never leaving it; others decode on the CPU. The result's `made_with` says which (`decoded on the GPU (NVDEC), AV1, NVIDIA hardware (NVENC)`). Its codec is the preferences' `proxies.codec`: `auto` (the default) is AV1 on a hardware encoder (NVENC, then VAAPI) when one works here, else H.264 with x264; `av1` falls back to SVT-AV1 instead; `h264` is x264. `vidcut preferences get` says what each choice uses on this machine. The codec is in the file name, so changing it makes new proxies. With no ids, every such clip in the pool. Naming a still or an audio-only file is an error; those are skipped when generating everything. It shows progress like an export, Ctrl-C stops it without leaving a file, and the GUI and other `vidcut` runs aren't held up while it encodes. In a running GUI the encode is a job, like an export: the Jobs window shows its progress and Cancel, and `vidcut jobs list` lists it (`kind: "proxy"`).
- Automatic proxies (preferences `proxies.*`, File > Preferences): while `use_proxies` is on, a running GUI makes them by itself for image sequences, videos over 3840×2160 and files over 150 Mb/s, after an import and when proxies are turned on; these jobs show `origin: "session"`. Headless runs never do. `vidcut proxy generate --auto` takes only what those rules pick; the proxy size default is `proxies.size`.
- `vidcut proxy remove [m1…]` deletes the proxy files and clears them. `vidcut project set --use-proxies true` plays fresh proxies in the preview (the Media Pool has the same switch). Export, `frame render`, analysis and OTIO keep the original paths. Preview audio stays on the original file.
- Proxy frames keep the source's presentation timestamps, so swapping the preview path stays frame-exact. `vidcut media get` reports the proxy path and `stale: true` when the source size or modification time no longer matches. Files live in `proxies/` beside the project (or `~/.cache/vidcut/proxies` when the project path is unknown).

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
  - Extra options: `--preset youtube-1080p`, `--encoder nvenc|vaapi|cpu|auto`, `--set video.quality=20`, `--range 1m..2m`, `--per-clip`, `--audio-only`, `--overwrite`.
  - With NVENC (and `gpu.acceleration` on), video pieces decode on NVDEC and deinterlace, scale and pad on the GPU, turned upright and BT.601, full range and HDR (tone mapping, HDR output) converted there by libplacebo (which also pads in 10 bits; HDR by a shader doing the CPU's math); the rest decode on the CPU and go up. The result's `gpu_frames` counts the frames made on the GPU (export.md §5).
  - `vidcut export plan out.mp4` (or `export start` with `--dry-run`) prints the plan and the chosen encoder without encoding.
  - Dynamic range and bit depth follow the footage (spec D44): `video.hdr` is `auto` (HDR when most of the shown picture is HDR and the codec carries it: HEVC, AV1, VP9), `sdr`, `pq` or `hlg`; an unset `video.bit_depth` is 10 for HDR, and for SDR when most of the picture has 10 bits or more (8 in H.264). Each output's `color` in the plan and the result says what was chosen and why (`hdr`, `bit_depth`, `picture`: frames of PQ, HLG and 10-bit picture, `why`). When HDR picture will be tone-mapped and SDR wasn't asked for, `notes` has `{"code": "hdr_tone_mapped", "message", "set"}`: `set` holds the `--set` overrides that keep it HDR (`video.codec=hevc`, `video.hdr=pq`).
  - Ctrl-C cancels cleanly (exit 9, no partial file).
  - With a GUI running, `--background` returns a job id at once; follow it with `vidcut jobs` commands (below).
- `vidcut presets list` lists the presets (`--json` includes their settings); `vidcut presets get NAME` shows one.
  - `vidcut presets save web --from youtube-1080p --set video.quality=22 [--quick-export]` saves a user preset to `~/.config/vidcut/presets/web.toml`; `--settings JSON|@FILE` gives full settings instead. `vidcut presets delete web` removes it.
- `vidcut encoders list` shows which GPU encoders work here (cached; `--refresh` re-tests), whether or not `gpu.acceleration` lets vidcut use them.
- `vidcut preferences get` shows the application preferences (`~/.config/vidcut/preferences.toml`) and what the thumbnail cache holds; `vidcut preferences set thumbnails.cache_limit_mb=256` changes them (`preferences.set` takes `{"thumbnails": {"disk_cache": false}}`; turning the cache off empties it). The cache holds the pool's pictures and the timeline's filmstrip tiles; `thumbnails.timeline_memory_mb` (256) is the memory a running GUI keeps tiles in.
  - `vidcut preferences set gpu.acceleration=false` (File > Preferences: GPU) turns off all GPU work: exports and proxies take software encoders whatever the settings or preset name (`export plan` shows `cpu`), so nothing decodes on NVDEC either, and the preview decodes in software at once: a running GUI watches the preferences file and reopens its players where they are (`player state` reports each player's `hwdec`). For comparing CPU and GPU pictures, or when a driver misbehaves. On by default.
- `vidcut frame render --at 1m --out f.png [--width 640]` renders what the timeline shows at a frame. `--media m1` renders a media frame instead. Use it to look at cuts.

**Automatic tools (spec D29–D32):**
- `vidcut edit ripple-delete-silence c1 [c2…] [--min 0.5s --threshold-db -40 --pre-head 0.15s --post-tail 0.15s]` (the defaults shown) cuts silent parts out of clips and closes the gaps, as one undo step. `--dry-run` lists the parts.
- `vidcut edit detect-scene-cuts c1` splits a clip where its shots change (`--threshold`, scene score 0–100, default 10).
- `vidcut analyze silence|scenes|black [--media m1 | --clips c1,c2]` and `vidcut analyze loudness [--range a..b]` measure without changing anything. Media positions are source frames; clip positions are timeline frames.
- `vidcut timeline boring [--boring 30s --jump 10f]` lists clips too long and too short (the Boring Detector).
- `vidcut storyboard render --out board.png [--every 5s --columns 4 --width 320]` draws a picture of every shot with timecode and clip id: the quickest way to look at a whole edit.
- `vidcut export start out.mp4 --set audio.loudness=-14` normalizes the mix to −14 LUFS (one gain, true peak ≤ −1 dBTP); the result reports the gain.
- `vidcut export start out.mp4 --set video.codec=hevc --set video.hdr=pq` (or `hlg`) exports HDR in 10 bits (HEVC, AV1 or VP9) whatever the footage; `video.hdr=sdr` tone-maps HDR sources to SDR. `vidcut media get` reports each clip's `hdr`, `rotation` and interlaced `fields`.
- `vidcut edit slide c3 --by 5f` moves a clip keeping its media; its neighbors give and take.
- Measuring decodes a file once; the results are cached (`~/.cache/vidcut/analysis/`), so trying other settings is quick.

**Timeline interchange (OpenTimelineIO, deviation X9):** files are written as DaVinci Resolve writes them, so they go to Resolve and back.
- `vidcut export start cut.otio` writes the whole timeline (or `--range`) for other editors: each video track, its clips' audio (J/L cuts included) as an audio track, each audio track, markers, and vidcut extras (marks, clip gain, track settings) in `metadata.vidcut`.
  - Media are plain absolute paths; source ranges count from each file's own start timecode (`vidcut media get` reports `timecode`); a clip's video and audio share a Resolve link group (`metadata.Resolve_OTIO`).
  - Image sequences go as `ImageSequenceReference`s whose available range counts the pictures at the sequence rate, and come back as sequences; a still is available for one frame, as Resolve writes it.
  - `cut.otioz` bundles the media too (a zip file; Resolve imports these), `cut.otiod` in a folder. Bundles copy whole media files, so in a GUI they run as jobs.
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
| `vidcut player state` | viewer mode, both playheads, the source clip, selection |
| `vidcut player seek 10s [--viewer timeline\|source_clip\|source_tape]` | returns once the frame is on screen (`CONFLICT` if someone moved the viewer first); `+10f`/`-2s` are relative |
| `vidcut player step [-]N`, `player play [--speed -2]`, `player pause`, `player toggle` | negative speeds play backwards |
| `vidcut viewer set source_clip\|source_tape\|timeline`, `viewer load m1 [--at 5s]` | what the viewer shows |
| `vidcut selection set --clips c1,c2 --media m3`, `selection get` | the selection |
| `vidcut gui screenshot --out v.png [--target window]` | the viewer picture as shown (works while minimized), or the whole window |
| `vidcut gui log [--since N]` | what everyone (the human too) did: each entry's `summary` in words, and the request as sent |
| `vidcut gui window --size 1600,1000`, `project open FILE [--create [--fps 25 --width 1920 --height 1080]]`, `gui quit` | window and project |
| `vidcut jobs list`, `jobs get\|wait\|cancel J [--timeout S]` | export and proxy jobs, the GUI's own included; `wait` fails like the job did |
| `vidcut pool set --bin A --sort name --descending true` | the bin the Media Pool shows and its order; Source Tape follows it |
| `vidcut player mute [true\|false]` | the viewer's monitoring mute (not the export); without a value, toggles |
| `vidcut mixer preview A1 -12` | a track's level heard now, without changing the project (`track set --volume-db` keeps one); `live: false` without ZeroMQ or FFmpeg's `azmq` |
| `vidcut gui show quick_export\|export\|jobs\|log\|mixer\|preferences` | opens that window for the person at the GUI |

- **Looking at the result:** `vidcut player seek 1m && vidcut gui screenshot --out shot.png` shows exactly what `vidcut frame render --at 1m --out f.png` renders. Both are frame-exact.
- **Exporting alongside the human:** `vidcut export start out.mp4 --background` returns at once; the export shows in the GUI's Jobs window, and `vidcut jobs wait j1` blocks until it's done. An export the human starts from Quick Export is a job too, so `vidcut jobs list` sees it.
- **Source marks and clipboard work headless too:**
  - `vidcut media marks set m1 --in 5s --out 10s`: edits without `--range` then use these marks.
  - Source Tape marks can span clips (spec D17): `vidcut tape marks set --in 200f --out 300f --order m1,m2` (tape frames of those media end to end), then edits with `--tape` instead of a media id, such as `vidcut edit append --tape`, place the span as one block.
  - `vidcut edit copy c3 --json`, then `vidcut edit paste --clips @clips.json --at 1m`.
- **Paths:** the CLI makes paths absolute before routing, those in `apply` batches too. In raw `rpc` requests, use absolute paths.

## Tools

**Fixtures:** generate one (cached, prints its path):

```bash
cargo run --release -p vidcut-av --example make_fixture -- hevc10 mkv 3840x2160 60 600
```

**GPU encoders:** `cargo run -p vidcut-av --example encoders` test-encodes every NVENC/VAAPI option, uncached.

**Other FFmpeg versions** (`scripts/ffmpeg-check.sh`, needs Docker): builds an image per target, then runs `cargo build --workspace` and `scripts/check.sh full` in it against the checkout (mounted read-only), and prints a summary.
- Targets: `lts` (Ubuntu 26.04's FFmpeg 8.0 and libmpv), `release` (the newest FFmpeg release, built from the official tarball on Ubuntu 26.04) and `master` (FFmpeg's git master, the same way; an early warning, allowed to fail). Without targets: `lts release`.
- `--fast` runs `scripts/check.sh` instead; `scripts/ffmpeg-check.sh release -- cargo test -p vidcut --test cli` runs any command, `-- bash` a shell.
- The containers have no GPU, display or audio device: the GPU, GUI and audio-device tests stay off (they are features), and outside `lts` the tests that need libmpv skip (Ubuntu's libmpv links Ubuntu's FFmpeg).
- FFmpeg is configured (`scripts/ffmpeg/build-ffmpeg.sh`) with what vidcut uses: x264, x265, SVT-AV1, libvpx, dav1d, Opus, LAME, Vorbis, zimg, ZeroMQ, libplacebo on Vulkan, VAAPI, NVENC/NVDEC and the CUDA filters (compiled by clang, no CUDA toolkit). Its build is a cached image layer: about a minute and a half on 16 cores, again only when `scripts/ffmpeg/` changes or master moves (`VIDCUT_FFMPEG_MASTER=COMMIT` pins one).
- Caches are Docker volumes: `vidcut-ffmpeg-cargo` (crates), `vidcut-ffmpeg-target-TARGET` and `vidcut-ffmpeg-data-TARGET` (fixtures made with that FFmpeg; the downloaded media are mounted read-only). A target directory takes about 5.5 GB and the images about 3 GB together. On 16 cores a first run of all three targets took 7 minutes, and a repeat run of both default targets takes about a minute. To free the space: `docker volume rm $(docker volume ls -q -f name=vidcut-ffmpeg)` and `docker image rm vidcut-ffmpeg:lts vidcut-ffmpeg:release vidcut-ffmpeg:master`.
- A newer FFmpeg release: put its version and the sha256 of its tarball, checked against its signature, in `scripts/ffmpeg/release.env` (the steps are there), then run `scripts/ffmpeg-check.sh release`.

**FFmpeg logging:** quiet by default (fatal only). Set `VIDCUT_AV_LOG=error` (or `warning`, `debug`) to see FFmpeg's messages when debugging media problems.

**Media inspection** (examples in `crates/vidcut-av/examples/`):
- `read_barcode FILE...`: the barcode index in images or video (compare screenshots with `vidcut frame render`).
- `packets FILE [FROM TO]`: video packets in decode order, with pts, dts and keyframe flags (open-GOP questions).
- `filmstrip FILE OUTDIR [N]`: pool thumbnails, timed.
- `tiles FILE [HEIGHT]`: timeline filmstrip tiles, timed at spacings from 0.25 to 30 s.

**Preview debugging:**
- `VIDCUT_DEBUG_PLAYER=1` traces every player command, mpv event, render and timeline gesture to stderr, and what the player engine (`vidcut_av::player`) decodes.
- `VIDCUT_MPV_OPTIONS="hwdec=no,ao=null"` overrides mpv options.
- `VIDCUT_MPV_LOG_DIR=DIR` makes each GUI player write mpv's own log, at debug level, to `DIR/<pid>-source.log` and `DIR/<pid>-timeline.log`. `VIDCUT_MPV_OPTIONS=msg-level=all=trace` adds more.
  - FFmpeg's messages are all in the timeline's log: both players' and those of vidcut's own FFmpeg work in the GUI process (thumbnails, waveforms, exports). FFmpeg has one log callback per process, and the first mpv instance (the timeline's) takes it.
  - `tests/gui.rs` sets it and `VIDCUT_DEBUG_PLAYER` for its GUI. A failed run keeps its temp dir, which the failure names: the screenshots, both mpv logs and the GUI's stderr.

**Preview spikes:**
- `cargo test -p vidcut-mpv --test edl_spike -- --nocapture` runs the mpv EDL/seek semantics experiments headless.
- `cargo run -p vidcut --example mpv_gl_spike -- FILE [FILE2] --auto-exit 8` opens a window with mpv embedded in egui and prints hwdec and drop statistics.

## Gotchas

The preview findings and the time-mapping rules are in [docs/decisions/preview.md](docs/decisions/preview.md); the export engine, sampling rule and encoder decisions are in [docs/decisions/export.md](docs/decisions/export.md); the player engine (threads, requests, the audio clock) is in [docs/decisions/player.md](docs/decisions/player.md). Read them before touching preview, seeking, time conversion, export or the player.

- **libmpv needs `LC_NUMERIC=C`.** `Mpv::new` calls `setlocale` before `mpv_create`.
- **mpv's render thread (the GUI's UI thread) may only call render-thread-safe client functions:**
  - async commands and property calls
  - `mpv_observe_property`, `mpv_wakeup`
  - `mpv_wait_event(…, 0)`

  Synchronous `command`/`get_property` there deadlocks against the video output. The mpv update and wakeup callbacks must not call mpv at all.
- **vsync must stay off** (`GlowConfiguration { vsync: false }`). On GNOME/Wayland with NVIDIA, a vsynced swap blocks forever while the window is hidden.
- **Start threads with `threads::spawn`, saying what the work is.** `Work::Job` (exports, proxies, measurements, thumbnails, anything decoding media nobody watches frame by frame) runs in Linux's idle scheduling class (below nice 19), and the FFmpeg threads it starts inherit that; `Work::Interactive` is for what someone waits on. A unit test fails on any other thread start in `crates/vidcut/src`.
- **Never lower a thread's priority with nice.** NVIDIA's driver, opening NVENC, copies the calling thread's nice value onto the process id, which is the main thread, and an unprivileged process can't raise it again: a proxy job at nice 19 left the GUI at nice 19. The idle class leaves the nice value alone.
- **A paused viewer shows vidcut-av's renderer, not mpv's picture** (preview.md §13): the same frame, decoded once whichever way it steps. Playing shows mpv. `gui.screenshot` captures mpv's picture, which differs by a couple of levels in fine detail. The renderer's tests count decoded frames with `vidcut_av::decode::frames_decoded()`, a process-wide count, so they stay one test in their own binary.
- **One CUDA device per process** (`cuda::CudaDevice::open`), with its Vulkan device: the Vulkan loader crashes (a null function in `vkEnumerateInstanceExtensionProperties`) when two threads make instances at once.
- **The player engine's clock is its audio output's callback** (player.md §4). Playing, the position is the sample the device has heard: with no output pulling, it never moves. Headless, give it a `PacedOutput`; a `ManualOutput` moves only when pulled. The device's data callback (`AudioSource::fill`) must never lock, allocate or decode.
- **libplacebo (turning, other matrices and HDR on the GPU, `cuda.rs`) keeps a frame back and snaps times within a millisecond to a frame.** Exports push a filler after each frame; the timestamps it sees are counts a second apart. Frames go in without side data (their display matrix would cancel the turn, HDR metadata would change the tone mapping), and 10-bit ones as P016, not P010, whose planes it reads and writes wrongly on a Vulkan device made from CUDA. It can't clear borders in FFmpeg's Vulkan frames, so it pads over a black frame. Its own HDR conversion isn't the CPU's (it lifts black for an SDR display, maps the curve on intensity, clips nothing): HDR goes through a user shader with the CPU's math, and libplacebo is told the picture is already in the output's colors. It keeps pictures between passes in half floats, stored rounded toward zero; the shader corrects by half a step (export.md §5).
- **FFmpeg's `scale` converts the matrix unless told it:** a graph whose source carries no colorspace gets BT.601 out of swscale, so the normalizer names the matrix for both sides, also where it only scales (before `zscale`). `zscale` can't write NV12 or P010; it writes their planar layouts, or FFmpeg converts its RGB with swscale (export.md §4).
- **Players render in `App::logic`, not `App::ui`.** eframe skips `ui` for hidden windows but always calls `logic`.
- **libmpv waits only 200 ms for the renderer to take a new frame.** Then it keeps the frame as its current picture and finishes the seek anyway, so "a new frame was rendered" doesn't prove a seek is on screen. The session counts a seek as shown only after the renderer presents the player following mpv's restart (docs/decisions/gui.md §4).
- **mpv's EDL drops leading pictures at segment ends.** Encodes that decode a keyframe before the frames shown just ahead of it lose those frames in the preview when a cut ends there. The live session patches such tails (preview.md §9). Test cuts against fixtures with B-frames and real media, not only small fixtures.
- **Timeline files pin mpv's audio output to the project's rate and channels** (`live::file_options`). At a cut into a clip with other audio, mpv otherwise drains and reopens the device, and the picture drops frames to catch up (preview.md §5).
- **Only the session thread calls mpv's client API.** Seeks while a file is still loading are rejected by mpv; `Player::seek` reloads instead.
- **File watching:** `notify` reports reads as access events. Ignore them, or a reload re-triggers itself forever.
- **The half-frame bias is applied exactly once.** mpv EDL `start` values are raw source timestamps and carry it, so timeline seeks to frame `j` go to exactly `j/f`.
- **Never send negative absolute seeks.** mpv counts them from the end of the file.
- **MPEG-TS and other index-less containers can't be seeked exactly by mpv.** Preview uses an MKV remux (`vidcut_av::remux`); export reads the original.
- **Set packet durations explicitly when muxing** from delayed encoders (x264 with B-frames). Otherwise MP4 edit lists drop the last frame.
- **Clear a decoded frame's picture type before encoding it** (`set_kind(picture::Type::None)`). Encoders take it as an order: the output copies the source's keyframes, and x264 warns about every B-frame it can't make.
- **Set `pkt_timebase` on every decoder** (`codec_ctx.pkt_timebase = stream.time_base`). Without it, FFmpeg can't adjust timestamps when it skips priming samples, and audio drifts by the codec delay.
- **Whether decoders output the priming samples depends on the FFmpeg version.** Matroska's codec delay (AAC, Opus) is decoded by FFmpeg 8.0 and skipped by 8.1 and later, which then count it in the stream's start; FFmpeg 9 also reports AAC priming in MP4 as `initial_padding`. `probe` subtracts what the first packet's skip-samples side data skips (`start_skips`), so an audio-only file's sound starts at 0 with any of them.
- **After the installed FFmpeg changes, run `cargo clean -p ffmpeg-sys-next`.** Its bindings come from FFmpeg's headers, and cargo doesn't rerun its build script when they change. The images of `scripts/ffmpeg-check.sh` do this by themselves.
- **Color goes through `vidcut_av::color`** (export.md §4): HDR sources are tone-mapped for SDR output, SDR sources sit at 203 nits in HDR output, untagged HD is BT.709. Rotation metadata is applied everywhere (spec D33).
- **`zscale` needs explicit output primaries** when converting to linear light: for untagged frames it otherwise fails with "no path between colorspaces".
- **Interlaced sources are deinterlaced inside `VideoReader`** (`bwdif`, one frame per frame; `bwdif_cuda` for frames NVDEC decodes), before frames are picked. The preview adds the same filter per file (preview.md §5); mpv's own `deinterlace` makes one frame per field, which breaks frame-exact seeks.
- **mpv's software renderer aborts on rotated video** (an assertion in `mp_image_crop`). The GUI renders with OpenGL, which rotates on the GPU, so `tests/live.rs` (software rendering) leaves rotated clips to `tests/gui.rs`.
- **GPUs on the dev machine:**
  - NVIDIA on `renderD128`: NVENC works, VAAPI doesn't.
  - AMD on `renderD129`: VAAPI H.264/HEVC.
  - The GL context is on NVIDIA, so preview hwdec is nvdec with zero-copy CUDA frames.
