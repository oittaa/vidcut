# GUI and live control

The viewer plays through the player engine ([player.md](player.md)), drawn with wgpu ([preview.md](preview.md)). What each panel shows and does: the spec. Commands: [cli.md](../cli.md).

**Tests** (`crates/vidcut/tests/`): `live.rs` (the live session, no window), `gui.rs` (the real window; `--features vidcut/gui-tests`, needs a display), `jobs.rs`, and `gui_*.rs` (panels under simulated input, egui_kittest). The JSON-RPC wire format: `ipc::tests` in `ipc/mod.rs`.

## 1. Threads

| Thread | Owns | Never does |
|---|---|---|
| **UI** (eframe on wgpu) | the egui context and its window; shows the viewer's targets as textures | draw a viewer picture, wait for the session |
| **Viewer** (`gui/video.rs`) | presents the player's engine: draws its picture, answers presents and viewer screenshots | wait for the UI |
| **Session** (`live`) | the open project, the player's engine, viewer/selection/clipboard state, the command log | touch the GPU |
| **Engine** (player.md §1) | decoding, the mix, the audio device | touch shared state |
| **IPC** | one listener plus one thread per connection | anything but forward requests and wait for answers |
| **Workers** (2) | thumbnails and waveforms (the timeline's tiles have their own, §11) | touch shared state (they report back by message) |
| **Tasks** | one thread per export job, frame render, screenshot encode or project-less request | touch the session's state |

- Everyone talks to the session through one `mpsc` inbox (`Handle`). After each batch of messages the session publishes an immutable `Arc<Snapshot>`, which the UI reads each frame.
- The session answers a request only once the published snapshot shows its result, so a client looking right after the answer never sees the state from before. Neither the UI nor the session ever blocks on the other.
- Only the session (when it publishes or needs the UI) and the viewer thread (when its picture changed) wake the UI. While playing, the session publishes at each new frame.

## 2. One request path

Every action is a [`Request`](../../crates/vidcut-core/src/request.rs), the type the headless CLI runs. GUI gestures send one each without waiting, and failures show as notices in the status bar; IPC clients wait for the answer. Project requests go through the project's `Editor`, exactly as headless.
- **The command log** (`gui.log`) shows every request with its origin (`gui`, `ipc`, `session`) and a `summary` in words (`vidcut_core::describe`), so the human sees what an agent did and the other way round. Scrub seeks (`fast`) and fader drags (`mixer.preview`) are left out.
- **Live-only requests** (`Request::is_live`: `viewer.*`, `player.*`, `selection.*`, `gui.*`, `jobs.*`, `mixer.preview`, `pool.set`, `project.open`) act on the session; run headless, they fail with `NO_INSTANCE`.
- **Slow requests** (`export.*`, `frame.render`, `storyboard.render`, `analyze.*`, project-less ones) run as tasks on a copy of the project taken when the request arrives, after its `if_rev` check. Media exports are jobs (§9).
- **Waiting requests** go on a wait list instead of blocking the session: `player.seek` and `player.step` until their frame is on screen (§4), `gui.screenshot` for the picture, `jobs.wait` for the job (§9). A wait gives up after 10 s (`jobs.wait` only with a `timeout`).

## 3. The project file stays the source of truth (deviation X1)

- Every committed change is saved before the reply, under the same lock headless runs take. The session watches the project's directory (`notify`) and reads changes by other processes back under that lock, so the project file and its history sidecar are always read together.
- `Store::save` writes the sidecar before the project file, so a headless edit keeps its undo label instead of looking like an outside change.
- If the sidecar can't be written, the edit is saved anyway with a warning; the old sidecar stays whole, and the GUI that made the edit keeps its history in memory.

## 4. When a frame is "on screen"

A seek, step, load or edit reload is a numbered **motion** of the player. It is on screen once the renderer has presented the player after the motion and shown the frame asked for:
1. The engine has the picture: the decode thread calls back when it lands (a kept picture is there at once), and the session asks the renderer (the viewer thread, in the GUI) to present.
2. The renderer presents (`Player::present`) and answers with the exact frame it showed. Paused, that must be the frame asked for; playing, any frame counts, since the clock starts there. A present that showed an older picture, or got no answer within 50 ms, is asked again.
- A seek also waits for the newest motion, because an edit's reload replaces what was asked for (at the same frame). So seek-then-screenshot is exact without sleeps.
- A picture that fails to decode fails the seek with `MEDIA` and the reason, and the viewer shows it.
- **Overtaken seeks fail with `CONFLICT`**, naming the newer frame: a waiting seek, step or viewer change whose frame a later seek replaces (another client, a scrub) would never show. Playback moving on and an edit's reload don't overtake it. The GUI's own overtaken seeks end as done, without an error.
- Every seek is exact; `fast` only keeps a scrub out of the command log, so a scrub to the same frame satisfies a waiting seek.
- **Held keys:** the engine merges seeks (player.md §2), so the picture moves at the speed frames decode and the last request lands exactly.

## 5. Rendering and screenshots

- The viewer thread draws the player's picture into a target, and the UI shows the target (preview.md §4). The viewer always shows the timeline's aspect, with sources fitted the way export fits them.
- A hidden window doesn't block seeks, viewer screenshots or edits (preview.md §2). Window screenshots and opening windows wait for the UI.
- `gui.screenshot` reads back the viewer's target by default (exactly as shown, also while hidden); `window` takes egui's viewport screenshot and needs the window to paint. PNG encoding runs on a task thread.

## 6. Live control: registry, socket, routing

- **Registry.** A running GUI writes `instances/<pid>.json` (pid, socket, open project, version) in the runtime directory: `$XDG_RUNTIME_DIR/vidcut`, else a private `vidcut-<uid>` in the temp directory, and `%TEMP%\vidcut` on Windows. Stale entries are removed when found.
- **Socket.** The GUI serves JSON-RPC 2.0 on `instances/<pid>.sock`, one JSON object per line. The directory is mode 0700, so only the user can connect.
- **Windows** (`ipc::transport`): a named pipe, named after the socket's path, stands in for the socket. Its access list grants the user alone, and a random number in its name keeps others from creating it first.
- **Limits** (`ipc::server::Limits`): request lines up to 64 MiB and 64 connections at once, so a runaway client can't take the session's memory or threads.
- **Routing** (`ipc::route`):
  - project-less commands (`Request::is_projectless`: `schema`, `probe`, `doctor`, `presets`, `encoders`, `preferences`, `project create`) and `--headless` run in the CLI process;
  - `--instance PID|SOCKET` (or `VIDCUT_SOCKET`) picks a GUI;
  - with `-p FILE`: the GUI with that project open, else headless, where live-only commands fail with `NO_INSTANCE`;
  - without `-p`: the only GUI with a project open; several is a usage error.
- Requests carry the project path, and a GUI with another project open refuses them (`CONFLICT`). The CLI makes path arguments absolute before routing, `apply` batches included.

## 7. Previews

- **One player.** Source Clip, Source Tape and the timeline each keep a playhead; one engine plays whichever the viewer shows, from the export's plans, proxies included (preview.md §1). The source viewer counts project frames (deviation X8).
- **Reloads** after edits are coalesced: one at once, then at most one per 100 ms while edits keep coming. Every live request applies a pending reload first, so it sees the edited timeline. A reload gives the engine the new plan (`set_plan`, player.md §3).
- **Stills and image sequences** play as export reads them; a sequence has exactly its own count of pictures, whatever else is in its folder. Import Media (`gui/import.rs`) asks once whether numbered runs become sequences (spec D39) or stills (deviation X17), in one request.

## 8. The timeline

The two timelines, tracks and their gestures: spec §5–§8. Mixer and meters: spec §9 and D28.
- **Gestures are pure functions** (`gui/timeline/geometry.rs`, `gesture.rs`, unit-tested there): what the pointer is over picks the operation, a `Preview` says what the release would do and the timeline draws it, and the release sends that one request.
- Gesture timing uses egui's clock, so tests can drive it. Icons (Phosphor) have their name as their accessible name, which tests find them by.
- **The Trim Editor** draws exact frames rendered by the export engine on a background thread (`gui/stills.rs`).
- **Waveforms** come from per-media peaks (100 per second, cached in `~/.cache/vidcut/waveforms/`), mixed to the project's channels by the export's resampler, so they are measured again when that count changes.
- **Meters** (`gui/mixer.rs`) read the cached waveforms at the playhead instead of tapping the player's output, so they work paused and while scrubbing and apply the export's gains, mutes and solos.
- **A fader** sends one `track.set` on release, so a drag is one undo step; while dragging, `mixer.preview` makes the level heard.
- **The pool view** (`pool.set`: bin, sort, direction) lives in the session, not the project, so Source Tape and agents see the same order. Pool drops send `media.move` or one `bins.rename`; Create Bin With Selected Clips is one `apply` batch, so one undo step.

## 9. Export windows and jobs

**Jobs** (`live/jobs.rs`): every media export and proxy encode in a running GUI, whoever started it. `.otio` exports write a small file at once and aren't jobs; `.otioz` and `.otiod` bundles copy whole media files, so they are.
- A thread per job prepares the export from a copy of the project (§2), reports its files (`JobPlanned`), then encodes, sending progress about ten times a second.
- **Foreground** (an IPC client without `background`): progress notifications carry the job id, the reply comes when the job ends, and closing the connection cancels it. Closing also cancels a `frame.render`, a storyboard, an analysis or the loudness measuring before an export.
- **Background** (`background: true`, and every GUI export): the reply comes once the files are planned, so mistakes still fail the request. Only `jobs.cancel` cancels it.
- `jobs.wait` is a wait-list entry (§2) and fails the way the job failed. `jobs.cancel` sets the job's cancel flag, and the engine removes partial files. Running jobs and the 50 most recent finished ones are kept.
- **Proxy encodes** (`proxy.generate`, spec D41) are jobs (`kind: "proxy"`): the encode runs on a copy of the project, then the request runs in the session and records the files. Automatic proxies are requested by the session itself (origin `session`).
- **Closing.** Quitting the window while jobs run asks first (`gui.quit` doesn't). Shutdown cancels running jobs and waits up to 3 s; a job still running is stuck in a decoder or an encoder, so the session removes its partial files itself, and waiting clients are told the GUI closed.

**CPU priority.** Every thread the app starts goes through `threads::spawn` and says what its work is (`Work`):
- `Work::Job` (exports, proxies, anything decoding whole files) runs at the lowest CPU priority, so the GUI, playback and other programs come first: Linux's idle class (inherited by FFmpeg's threads), Windows's idle priority, macOS's utility QoS class (neither inherited). Why not nice 19 or the background class: `threads.rs`.
- `Work::Interactive` (the session, requests, file dialogs, frames and plans) and headless runs keep normal priority: someone waits for them.

**The windows only send requests:**
- The Export dialog starts from a preset, and what the user changes becomes `--set` overrides of it, so every GUI export is `vidcut export start OUT --preset NAME --set …`, which the dialog shows. Quick Export (spec §10) lists the presets marked `quick_export`.
- The dialog's plan is `export.plan`, asked again whenever the params change (one request in flight). Plans never change anything: per-clip folders are made when the export runs.
- Windows that need answers call `Handle::call` on a helper thread and poll the result each frame (`Ask`). These calls stay out of the command log, and their errors show in the window. File dialogs run on helper threads behind a `Dialogs` trait, which tests answer directly.
- Import Timeline (deviation X9) unpacks an `.otioz` on a worker first (`Step::Measure`), so the session keeps answering meanwhile.
- `gui.show` opens any of these windows (or the log, or the Mixer) for the person at the GUI.

## 10. Automatic tools

- **Ripple Delete Silence:** the dialog (`gui/timeline/tools.rs`) dry-runs `edit.ripple_delete_silence` on a helper thread whenever its settings change, and Remove sends the request once. The session measures media on a worker before such an edit runs (agents.md §3), so the GUI stays live while a long clip decodes.
- **Boring Detector:** its highlights are computed each frame from the project (`analysis::boring`, which is cheap).
- **Dynamic Trim** (`dynamic_keys`) counts frames on egui's clock. Each frame it tries the next count on a copy of the project, stopping where an edit would fail, and the viewer shows the two-up rendered from that copy.

## 11. Thumbnails and preferences

**Pool filmstrips** (`thumbcache.rs`): 24 pictures of a media file, fitted in 192×108, stored as one JPEG strip in `~/.cache/vidcut/thumbs`. They are on disk because decoding them is slow, especially from a network share; the project still holds no pictures (deviation X1).
- The file name holds the media file's path hash, size and modification time and the strip's layout, so nothing reads a stale strip. A strip's modification time is its last use; after each new strip, the least recently used go until the rest fit `thumbnails.cache_limit_mb`.

**Preferences** (`preferences.toml`, deviation X12) are read whenever a thumbnail job, export or proxy runs, and the session watches the file, so a change from anywhere reaches a running GUI at once:
- the engine switches its decoder (preview.md §3) or reopens on the new audio device (player.md §6), a smaller player memory limit drops pictures at once, and the thumbnail cache empties when turned off and trims when its limit drops.

**Timeline filmstrips** (`gui/tiles.rs`): each tile shows the frame under it, since the pool's 24 pictures per file are too sparse.
- Tiles show frames on a grid whose step is the largest power of two frames no wider than a tile, so zooming in or out by two keeps half of them.
- Four workers at job priority (§9) make the tiles painting asked for, in its order, from the export's sampling (`FrameReader`) and a fresh proxy when there is one.
- With `gpu.acceleration` and `gpu.decoding` on, HEVC, AV1 and VP9 decode and shrink on the GPU. H.264 stays on the CPU: it decodes cheaply there, and every seek recreates NVDEC's H.264 decoder (gotchas.md). A small integrated GPU beside a fast CPU can be slower; `gpu.decoding` off then helps.
- Pictures are kept as textures up to `thumbnails.timeline_memory_mb`, and on disk in the thumbnail cache in blocks of 64 source frames, so a reopened project fills its tiles from disk.
