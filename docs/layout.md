# Workspace layout

| Crate | Purpose | Depends on |
|---|---|---|
| `crates/vidcut-core` | Project model, time, edit operations, the request protocol, render plans, OTIO, analysis decisions, preferences, fingerprints and the SHA-256 digests that name and check vidcut's own files, HDR metadata choice, where files live on each system (`dirs.rs`). Pure Rust. | — |
| `crates/vidcut-av` | FFmpeg in-process: probe, decode (on the CPU, or the GPU's decoders: `hw/`, on the GPU in use: `gpus.rs`), where each frame goes between the GPU and memory (`route.rs`) and how its picture is made on the way (`export/make.rs`; a dissolve's pictures mixed in `export/mix.rs`, and in `cuda/mix.rs` on NVIDIA's GPU; NVDEC's kept pictures in memory of their own, `cuda/keep.rs`; the CPU's decoded frames in buffers of their own, `buffers.rs`), HDR, export, proxies, thumbnails, measurements, the player engine, the shared pools that run slices (`pool.rs`). [player.md](decisions/player.md), [export.md](decisions/export.md), [preview.md](decisions/preview.md). | ffmpeg-next, cpal |
| `crates/vidcut` | The `vidcut` binary: Session/host, IPC, CLI (clap), GUI (eframe on wgpu), MCP server | all of the above |

Inside `crates/vidcut`:

| Module | What |
|---|---|
| `cli.rs`, `main.rs`, `output.rs` | the command line, generated from the request schema (method `group.verb` is `vidcut group verb`) → one `Request`; human/JSON output; routing |
| `mcp.rs` | `vidcut mcp`: the MCP server (tools generated from the request schema) |
| `storyboard.rs` | `storyboard.render`: a labeled PNG grid of the program's shots |
| `env.rs` | `AvEnv`: probing and the measurements behind the automatic tools, cached |
| `session.rs` | runs requests headless against a project file |
| `themes.rs`, `themes/` | the GUI's themes: the built-in TOML files and the user's |
| `look.rs` | the computer's light or dark mode and accent color, which native themes follow |
| `preferences.rs`, `thumbcache.rs` | `~/.config/vidcut/preferences.toml` and the pool thumbnails kept in `~/.cache/vidcut/thumbs` |
| `threads.rs` | the one way the app starts threads: `threads::spawn(name, Work::Job \| Work::Interactive, f)`; media work runs at the lowest CPU priority |
| `live/` | the GUI's core thread: open project, the viewer's player (`player.rs`: one engine for the source and the timeline, the frame of the one not shown kept ready; plans, motions, when a frame is on screen), viewer state, waits, command log, export jobs (`jobs.rs`), which media files are missing (`missing.rs`, on a thread of its own); publishes `Snapshot`s |
| `ipc/` | JSON-RPC over a Unix socket, or a named pipe on Windows (`transport.rs`), the instance registry, CLI routing |
| `gui/` | `app.rs`: eframe glue; `video.rs`: the viewer thread presenting the player; `gate.rs`: keeps the viewer thread's submissions out of the window's surface configures; `picture.rs` with `picture.wgsl`: a YUV picture drawn by the GPU shader into the viewer's target, read back for screenshots (works without a window); `shared.rs`: the Vulkan memory CUDA copies NVDEC's pictures into; `vulkan.rs`: the images wgpu's device makes of the memory of pictures FFmpeg decoded on Vulkan; `workspace.rs`: all panels and UI state (testable without a GPU); `timeline/`: geometry, gestures, drawing; `export.rs`: Quick Export, the Export dialog, jobs and OTIO windows; `mixer.rs`: the Mixer and meters; `preferences.rs`: File > Preferences; `project.rs`: New Project's format and File > Project Settings; `timeline/tools.rs`: Ripple Delete Silence, the Boring Detector, Dynamic Trim; `tiles.rs`: the timeline's filmstrip pictures, each tile's own frame; `folders.rs`: where file dialogs start; `file_manager.rs`: Show in Folder; `relink.rs`: Relink Media and its result; `cells.rs`: the two-up's two players and their pictures; `pool.rs` (with bins), `viewer.rs`, `video.rs` |
| `export.rs` | presets (built-in and user), outputs, export plans and timeline (`.otio`, bundle) exports as the session runs them |
| `bundle.rs` | OpenTimelineIO bundles: writing `.otioz` (zip) and `.otiod` (folder) with their media, unpacking `.otioz` for import |

The index of the docs is [AGENTS.md](../AGENTS.md).
