# AI agents: MCP, measurements and storyboards

## 1. MCP server (`vidcut mcp`, `crates/vidcut/src/mcp.rs`)

- **Transport:** MCP's stdio transport (JSON-RPC 2.0, one message per line), hand-written with no tokio or MCP SDK, since there is nothing async to do. It answers with the client's protocol version when it knows it.
- **Every request method is a tool** (`.` becomes `_`), so agents can do everything the CLI and GUI can; a curated subset would drift from the protocol. Input schemas are the methods' params schemas from schemars, made self-contained. `use_project`, the one MCP-only tool, picks or creates the project file.
- **Calls run like CLI commands** (`ipc::run`, shared with `main.rs`): in the GUI that has the project open, or headless, with file parameters made absolute first. A failure is a tool error whose text starts with the stable error code.
- `frame.render`, `gui.screenshot` and `storyboard.render` also return their PNG as image content, so an agent can look at what it made.
- **Cancelling:** each call runs on its own thread, its cancel flag registered before it starts, so `notifications/cancelled` reaches it. A call routed into the GUI is cancelled by closing its connection. A background export is a job, which only `jobs.cancel` cancels. A cancelled request gets no answer, as MCP requires.
- **Progress:** with a `progressToken`, exports send `notifications/progress`.

Tests: `crates/vidcut/tests/mcp.rs`.

## 2. Measurements (`vidcut_core::analysis`, `vidcut_av::analysis`)

- The media layer measures; the core decides. The editor's `Env` supplies `audio_levels` (per stream, the peak of all channels in 10 ms steps), `picture_stats` (each frame's scene score and black share) and `loudness` (of a render plan's mix, FFmpeg's `ebur128`).
- Everything else (silences, cut frames, black stretches, the Boring Detector, the frame mapping) is plain arithmetic in vidcut-core, tested without media.
- Measurements don't depend on thresholds and are cached in the process and on disk (`~/.cache/vidcut/analysis/`), so an agent can try settings without decoding again. Pictures are measured small (spec D30).

## 3. In the live session

- **Edits that measure** (`edit.ripple_delete_silence`, `edit.detect_scene_cuts`) must not block the session thread while a stream decodes: the session measures on a worker first (`Step::Measure`), then runs the edit in order, as one undo step.
- Analysis queries and storyboards run on a worker against the saved project, like exports.
- GUI previews (Ripple Delete Silence's red stripes) are dry runs: the command log leaves them out, and they never count as a change.

## 4. Storyboards (`crates/vidcut/src/storyboard.rs`)

- One picture per shot (the middle frame of each stretch the render plan takes from one source) or one every N frames, rendered by the export engine, at most 200.
- Labels (record timecode, clip id, the track if not V1) use a built-in 5×7 pixel font, so no font files or text libraries are needed.

## 5. Loudness normalization (export)

An export with `audio.loudness` first measures the mix it is about to encode, then applies one gain toward the target, held so the true peak stays at or below −1 dBTP. A silent mix gets no gain. Tests: `crates/vidcut/tests/cli.rs`.
