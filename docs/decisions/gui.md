# GUI and live control: decisions from M3

**Status:** accepted (M3, October 2026).

**Evidence** (all rerunnable):
- [`crates/vidcut/tests/live.rs`](../../crates/vidcut/tests/live.rs): the live session with software-rendered players and no window. Covers frame-exact seeks at every boundary, the source viewers, the clipboard, edits from other processes, and errors.
- [`crates/vidcut/tests/gui.rs`](../../crates/vidcut/tests/gui.rs) (`--features vidcut/gui-tests`, needs a display): the real GUI driven through the CLI. Covers edit latency, screenshot vs `vidcut frame`, and a minimized window.
- `ipc::tests` in [`crates/vidcut/src/ipc/mod.rs`](../../crates/vidcut/src/ipc/mod.rs): the JSON-RPC wire format.

## 1. Threads

| Thread | Owns | Never does |
|---|---|---|
| **UI** (eframe) | the egui context, the GL context, mpv **render** contexts and their textures | call mpv's client API, wait for the session |
| **Session** (`live`) | the open project, both mpv client handles, viewer/selection/clipboard state, the command log | touch GL |
| **IPC** | one listener plus one thread per connection | anything but forward requests and wait for answers |
| **Workers** (2) | thumbnails, preview remuxes, tail checks and patches | touch shared state (they report back by message) |
| **Tasks** | one thread per export, frame render, screenshot encode or project-less request | touch the session's state |

**How they talk:**
- **One inbox.** Everyone talks to the session through one `mpsc` inbox (`Handle`).
- **Snapshots out.** After each batch of messages, the session publishes an immutable `Arc<Snapshot>`, and the UI reads the latest one each frame. Publishing is a pointer swap under a mutex.
- **No blocking.** The UI therefore never blocks on the session, and the session never blocks on the UI.
- **Repaints.** The session calls `request_repaint` when it publishes, and mpv's render update callback does too. Nothing else wakes the UI.

## 2. One request path

Every action is a [`Request`](../../crates/vidcut-core/src/request.rs), the same type the headless CLI runs:
- **GUI gestures** send one request each, without waiting for an answer. Failures show as notices in the status bar.
- **IPC clients** send a request and wait for its answer.
- **The command log** shows every request with its origin (`gui`, `ipc`), so a human can see what an agent did and an agent can read what the human did (`gui.log`). Fast scrubbing seeks are left out.

**Kinds of request:**
- **Project requests** go through the project's `Editor`, exactly as headless.
- **Live-only requests** act on the session: `viewer.*`, `player.*`, `selection.*`, `gui.*` and `project.open`. Run headless, they fail with `NO_INSTANCE`.
- **Slow requests** run as tasks on a copy of the project: `export.*`, `frame.render`, and project-less ones such as `encoders.list`. Export progress is streamed to the client, and closing the connection cancels the export.

**Requests that wait** don't block the session. They become entries in a wait list:
- **`player.seek` and `player.step`** wait until the frame is on screen (§4).
- **`gui.screenshot`** waits for the picture.
- **Limit.** Any wait gives up after 10 s.

## 3. The project file stays the source of truth (deviation X1)

**Saving.** Every committed change is saved before the reply, under the same lock headless runs take.

**Watching.** The session watches the project's directory (`notify`, non-recursive, ignoring access events). Changes made by other processes are read back under the lock, so the project file and its history sidecar are always seen together.

**Write order.** `Store::save` writes the history sidecar before the project file. The new state then never shows up without its history. Otherwise a headless edit would be mistaken for an "external change" and lose its undo label.

**Locking.** The lock is retried every 5 ms. An edit made by another process shows up in the GUI within about 6 ms (`tests/live.rs`).

## 4. When a frame is "on screen"

mpv runs async commands in order. A reply to command `n` therefore means every command up to `n` has been queued.

A seek or load ("motion") is shown when two things have happened:
1. a `playback-restart` arrived after the motion's reply;
2. the renderer drew a **new picture** since the motion was issued.

"New picture" comes from `MPV_RENDER_PARAM_NEXT_FRAME_INFO`: present, and not a redraw or repeat. While paused, the only new picture after a seek is the seek's own frame.

**The older rule failed.** We first counted any render after the restart, but the render can happen before the session sees the restart event. The fallback then made seeks take 250 ms.

**Fallback.** If no new picture comes within 250 ms of the restart, the motion counts as shown anyway. This covers no renderer, the same picture again, or a window that renders only every 100 ms.

**More waiting.** A seek also waits for the player's newest motion (a preview reload replaces what was asked for) and for pending tail patches (preview.md §9). Seek-then-screenshot is therefore exact without sleeps. Over IPC it takes about 10–15 ms.

**Seeks while loading.** mpv rejects seeks until a freshly loaded file is open. A seek in that window opens the file again at the new position instead.

## 5. Rendering and screenshots

**Rendering.**
- Both players render in `App::logic`, not `App::ui`. eframe calls `logic` even when it skips `ui` for a hidden window, so mpv never stalls waiting for us.
- On Wayland, winit throttles redraws only through frame callbacks that eframe never arms. With vsync off, the GUI keeps painting while minimized, and live control keeps working (`tests/gui.rs`).

**`gui.screenshot` targets:**
- **`viewer`** (the default) reads the player's render target back: the picture exactly as the viewer shows it, at the viewer's size. It works while the window is hidden.
- **`window`** uses egui's viewport screenshot of the whole window. It needs the window to paint.

**Encoding.** PNG encoding runs on a task thread.

**Aspect.** The viewer shows the timeline's aspect in every mode, and sources are fitted inside it the way export fits them. Render targets are sized to the fitted picture in physical pixels.

## 6. Live control: registry, socket, routing

**Registry.** A running GUI writes `$XDG_RUNTIME_DIR/vidcut/instances/<pid>.json` with its pid, socket, open project and version. It serves JSON-RPC 2.0 on `<pid>.sock`, with one JSON object per line (format in `ipc/mod.rs`).
- **Access.** The directory is mode 0700, so only the user can connect.
- **Clean-up.** Stale entries, and the per-process preview directories of dead instances, are removed when they're found.

**Routing.** The CLI routes each command:
- **Project-less commands** (`schema`, `probe`, `presets`, `encoders`, `doctor`) always run in the CLI process.
- **`--headless`** runs in the CLI process.
- **`--instance PID|SOCKET`** (or `VIDCUT_SOCKET`) picks a GUI.
- **With `-p FILE`:**
  - the GUI with that project open, if any;
  - otherwise headless, except live-only commands, which fail with `NO_INSTANCE`.
- **Without `-p`:** the only GUI with a project open; with several GUIs, it's a usage error.
- **Safety check.** Requests carry the project path, and a GUI that has a different project open refuses them (`CONFLICT`).

**Paths.** The CLI makes path arguments absolute before routing, since the GUI may run in another directory. Raw `rpc` requests should use absolute paths.

## 7. Previews

**Players.**
- **Source player:** Source Clip and Source Tape share it.
- **Timeline player:** the program.
- Switching modes pauses the player being left, and each content type keeps its own playhead.

**What they load.** All three viewer modes play EDLs built by the same code: `render::tape` for sources and `render::flatten` for the timeline. Source time therefore maps to frames exactly as the timeline's does: the source viewer counts project frames (deviation X8).

**Reloads.** Timeline reloads are coalesced: one at once, then at most one per 100 ms while edits keep coming. A seek flushes a pending reload first.

**Fillers.** Fillers are generated on demand, synchronously (once per size and rate, then cached), so the preview is never half-loaded.

**Pool thumbnails.**
- **Frames:** 24 per clip at 192×108, spread evenly.
- **Accuracy:** each is within half the spacing of its time, so keyframes are good enough for long clips; short clips are decoded forward.
- **Speed:** ~0.1–3 s per clip on workers.

**Stepped reverse.** Reverse shuttle (J) uses exact seeks, one in flight at a time, at the shuttle speed (preview.md §7).

## 8. The timeline (M4)

**Two timelines** (spec §5):
- **Upper:** the whole program at full width.
- **Lower:** a detail view at a zoom you choose (Ctrl+wheel, −/+), its playhead fixed in the middle unless Timeline Options says otherwise.
- **Shared drawing.** Both draw the same clips: names, durations, status, filmstrips from the pool thumbnails, waveforms from per-media peaks (100 per second, cached in `~/.cache/vidcut/waveforms/`).

**Gestures are pure functions** (`gui/timeline/geometry.rs`, `gesture.rs`):
- What the pointer is over picks the operation: edge, edit center, slip handle, audio edit, body.
- While dragging, a `Preview` says what the release would do. The timeline draws it: the target, the moved block, the snap line, a tooltip.
- The release sends that one request. Scrubs only seek.
- **Clock.** Gesture timing (the 0.8 s hover, Safe Edit's 0.6 s) uses egui's clock, so tests drive it.

**Tests:**
- `geometry.rs` and `gesture.rs` unit tests cover zones, snapping and every drop rule.
- `tests/gui_input.rs` (egui_kittest, no window) clicks, drags and types into the real panels against a live session. It checks that each gesture changes the project with exactly one request and that the command log matches the undo history. It also times frames with ~2,000 clips: about 2.5 ms of UI work in a debug build.

**The Trim Editor** draws exact frames rendered by the export engine on a background thread (`gui/stills.rs`).

## 9. Still open

- **Pool sort order and bins**, which also set the Source Tape order. They come with M4/M6.
- **Source Tape marks spanning several clips** (decision D17 limits marks to one clip).
- **Audio meters and monitoring mute** come with the mixer (M6).
