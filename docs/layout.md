# Workspace layout

| Crate | Purpose | Depends on |
|---|---|---|
| `crates/vidcut-core` | Project model, time and timecode math, edit operations (`ops.rs`; tracks in `ops/tracks.rs`, clips following Track 1 in `ops/follow.rs`), Request/Response protocol, Editor (undo/rev), project store, render plans (`render.rs`: top-wins video, mixed audio lanes), the proxies the viewer plays (`preview.rs`), OpenTimelineIO reading and writing (`otio.rs`, tests in `otio/tests.rs`), what the automatic tools decide from measurements (`analysis.rs`), application preferences (`preferences.rs`), media fingerprints for relinking (`fingerprint.rs`) | — (pure Rust; fast to build and test) |
| `crates/vidcut-av` | FFmpeg layer: probe (start timecodes too), exact-seek decode (on NVDEC too, `cuda.rs` for the GPU pipeline, `cuda/share.rs` for memory a display shares, NVDEC's pictures copied into it on the GPU; `VideoReader::open_nvdec_or_cpu` decodes on NVDEC when it can), encode/mux, export engine (with loudness normalization; the next piece's file opens ahead in `export/ahead.rs`), the renderer of two-up pictures that decodes each frame once either way, and the frame reader of timeline tiles (`export/renderer.rs`), thumbnails (and their JPEG cache form), waveforms, still frames, measurements for the automatic tools (`analysis.rs`), the export's color math (`color.rs`, `shader_colors` for the viewer's shader), encoder detection, test-fixture generator, the player engine (`player/`: a decode thread keeping decoded YUV pictures ahead of the clock and windows for going back, on NVDEC when there is one, the frames copied down, the audio mix into a lock-free ring, reversed backward and time-stretched at other speeds keeping the pitch (`player/stretch.rs`, which marks each block for the clock), the device callback as the clock, outputs through cpal, audio scrubbing in `player/scrub.rs` with `Scrubber` for use without a player, a frame of the plan played next prepared and held) | ffmpeg-next, cpal |
| `crates/vidcut` | The `vidcut` binary: Session/host, IPC, CLI (clap), GUI (eframe on wgpu), MCP server | all of the above |

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
| `live/` | the GUI's core thread: open project, the viewer's player (`player.rs`: one engine for the source and the timeline, the frame of the one not shown kept ready; plans, motions, when a frame is on screen), viewer state, waits, command log, export jobs (`jobs.rs`), which media files are missing (`missing.rs`, on a thread of its own); publishes `Snapshot`s |
| `ipc/` | JSON-RPC over a Unix socket, the instance registry, CLI routing |
| `gui/` | `app.rs`: eframe glue; `video.rs`: the viewer thread presenting the player; `picture.rs` with `picture.wgsl`: a YUV picture drawn by the GPU shader into the viewer's target, read back for screenshots (works without a window); `shared.rs`: the Vulkan memory CUDA copies NVDEC's pictures into; `workspace.rs`: all panels and UI state (testable without a GPU); `timeline/`: geometry, gestures, drawing; `export.rs`: Quick Export, the Export dialog, jobs and OTIO windows; `mixer.rs`: the Mixer and meters; `preferences.rs`: File > Preferences; `project.rs`: New Project's format and File > Project Settings; `timeline/tools.rs`: Ripple Delete Silence, the Boring Detector, Dynamic Trim; `tiles.rs`: the timeline's filmstrip pictures, each tile's own frame; `folders.rs`: where file dialogs start; `relink.rs`: Relink Media and its result; `pool.rs` (with bins), `viewer.rs`, `video.rs`, `stills.rs` |
| `export.rs` | presets (built-in and user), outputs, export plans and timeline (`.otio`, bundle) exports as the session runs them |
| `bundle.rs` | OpenTimelineIO bundles: writing `.otioz` (zip) and `.otiod` (folder) with their media, unpacking `.otioz` for import |

Docs:
- `docs/cut-page-spec.md`: the behavioral spec.
- `docs/deviations.md`: intentional differences from Resolve.
- `docs/roadmap.md`: the use cases vidcut is for, and the order of what comes next.
- `docs/todo/`: open work, one file per item.
- `docs/decisions/`: short architecture decision records (preview, export, GUI and live control, agents: MCP and measurements, the player engine).
