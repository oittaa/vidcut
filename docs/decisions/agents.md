# AI agents: MCP, measurements and storyboards (M7)

**Status:** accepted (M7, October 2026).

## 1. MCP server (`vidcut mcp`, `crates/vidcut/src/mcp.rs`)

**Transport:** MCP's stdio transport: JSON-RPC 2.0, one message per line. Protocol versions 2025-06-18, 2025-03-26 and 2024-11-05; the server answers with the client's version if it knows it. Hand-written: there is nothing async to do, so no tokio or MCP SDK.

**Tools are the requests:**
- Each `Request` method is a tool named with `_` for `.` (`edit.ripple_delete_silence` → `edit_ripple_delete_silence`). Its input schema is the method's params schema from schemars, made self-contained: a params type that is a definition of its own is inlined, and the definitions it uses are copied into the tool's `$defs`.
- `schema` isn't a tool (`tools/list` gives every schema). `use_project` is the one MCP-only tool: it picks the project file (or makes one, with an optional fps and size) for later calls.
- All 97 request methods are tools (98 with `use_project`), about 68 KB of `tools/list`. Agents get everything the CLI and GUI can do. A curated subset would drift from the protocol.

**Calls run like CLI commands** (`ipc::run`, shared with `main.rs`):
- **Routing:** a call goes into the GUI that has the project open (`via: gui:<pid>`), or runs headless. `vidcut mcp --headless` and `--instance` route like the CLI's flags.
- **Paths:** file parameters, those of `apply`'s commands too (`Request::paths_mut`), are made absolute first, since a GUI may run the call from another directory.
- **Results:** the vidcut response (`rev`, `changes`, `result`, `via`) comes back as text and as structured content. A failure is a tool error (`isError`) whose text starts with the stable error code (`INVALID_EDIT: ...`).
- **Pictures:** `frame.render`, `gui.screenshot` and `storyboard.render` also return their PNG as image content (up to 8 MB), so an agent can look at what it made.
- **Concurrency:** each call runs on its own thread, so `notifications/cancelled` can reach a running one. Its flag is registered before the call starts, so an immediate cancel isn't lost.
- **Cancelling:** a headless export polls the cancel flag. A call routed into the GUI closes its connection. The GUI sets that call's cancel flag when a progress write fails, and also when the connection closes while nothing is reporting progress. The same flag stops a foreground export (its frame loop and the loudness measure before it), `frame.render`, a storyboard and an analysis. A background export is a job: the client leaving does not cancel it, `jobs.cancel` does. A cancelled request gets no answer, as MCP asks (`tests/mcp.rs` cancels a slow 4K export in the GUI).
- **Progress:** with a `progressToken`, exports send `notifications/progress` (frames done of total, about four a second).

## 2. Measurements (`vidcut_core::analysis`, `vidcut_av::analysis`)

**Split:** the media layer measures and the core decides.
- The editor's `Env` (`AvEnv`, or the tests' `FakeEnv`) supplies three measurements:
  - `audio_levels`: per stream, the peak of all channels in 10 ms steps
  - `picture_stats`: per video, the scene score and black share of each frame, with its timestamp
  - `loudness`: of a render plan's mix (FFmpeg's `ebur128` filter)
- Everything else is plain arithmetic in vidcut-core and tested without media: silences, cut frames, black stretches, the Boring Detector, the parts to cut, and the frame mapping (export's sampling rule).

**Measurements are kept:**
- **Where:** in the process and on disk (`~/.cache/vidcut/analysis/`, keyed by path, size and modification time).
- **Why:** they don't depend on thresholds, so an agent can try settings quickly. Decoding a whole stream is the slow part: Big Buck Bunny's 10.5 minutes of 1080p30 take about 10 s.

**Pictures are measured small** (96×54 luma), enough to tell shots apart. scdet's score (spec D30) found Big Buck Bunny's cuts on their exact frames. They decode on the GPU where it decodes the stream (preview.md §3) and come down whole, so the scores are the CPU's to the bit.

## 3. In the live session

- **Edits that measure** (`edit.ripple_delete_silence`, `edit.detect_scene_cuts`) must not block the session thread while a stream decodes. The session first measures on a worker (`Step::Measure`), then runs the edit as usual. The measurements are then in memory, so the edit takes milliseconds. The request still applies in order, as one undo step.
- **Analysis queries and storyboards** run on a worker against the saved project, like exports and `frame.render`.
- **GUI previews** (Ripple Delete Silence's red stripes) are dry runs. The command log leaves them out, and a dry run never counts as a change.

## 4. Storyboards (`crates/vidcut/src/storyboard.rs`)

- **Pictures:** one per shot (the middle frame of each stretch the render plan takes from one source), or one every N frames. Each is rendered by the export engine (`still::rgba`) on up to six threads.
- **Layout:** a grid with a label under each picture: record timecode, clip id, and the track if not V1. The labels use a built-in 5×7 pixel font, so no font files or text libraries are needed. The result lists every cell's frame, clip and position.
- **Limit:** at most 200 pictures; use `every` or `range` for more.

## 5. Loudness normalization (export)

- **Measure, then gain:** an export with `audio.loudness` measures the mix it is about to encode (same lanes, channels and de-click), then applies one gain toward the target. The gain is held so the true peak stays at or below −1 dBTP. A silent mix gets no gain.
- **Reported:** the export reports the measured loudness, the gain, and the loudness and true peak that result.
- **Verified:** a −20 LUFS tone exported at −16 LUFS measures −16 afterwards. Beeps that would clip stop at −1 dBTP (`tests/cli.rs`).
