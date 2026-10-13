# GUI and live control

The viewer plays through the player engine ([player.md](player.md)), drawn with wgpu ([preview.md](preview.md)).

**Tests** (`crates/vidcut/tests/`): `live.rs` (the live session, no window), `gui.rs` (the real window; `--features vidcut/gui-tests`, needs a display), `jobs.rs`, and the `gui_*.rs` files (the panels under simulated input, egui_kittest). The JSON-RPC wire format: `ipc::tests` in `ipc/mod.rs`.

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

**How they talk:**
- **One inbox.** Everyone talks to the session through one `mpsc` inbox (`Handle`).
- **Snapshots out.** After each batch of messages, the session publishes an immutable `Arc<Snapshot>` (a pointer swap under a mutex). The UI reads the latest one each frame.
- **Answers after the snapshot.** The session answers a request only once the published snapshot shows its result. A client that looks right after the answer (the GUI, a test stepping it) never sees the state from before.
- **No blocking.** The UI never blocks on the session, and the session never blocks on the UI.
- **Repaints.** Only two threads wake the UI (`request_repaint`): the session when it publishes or needs the UI (a capture of the window, a window change), and the viewer thread when the picture it drew changed. While playing, the session publishes each time the clock reaches a new frame.

## 2. One request path

Every action is a [`Request`](../../crates/vidcut-core/src/request.rs), the same type the headless CLI runs:
- **GUI gestures** send one request each and don't wait for the answer. Failures show as notices in the status bar.
- **IPC clients** send a request and wait for its answer.
- **The command log** shows every request with its origin (`gui`, `ipc`, `session`). A human sees what an agent did, and an agent reads what the human did (`gui.log`).
  - Each entry has a `summary` in words (`vidcut_core::describe`): times as timecodes, media by name, and for undo or redo the steps it took, named as the undo history names them. The GUI shows the summary, and the request on hover.
  - Left out: scrub seeks (`fast`) and a fader's drag (`mixer.preview`). The GUI sends a switch to the timeline view only when the viewer isn't on it already.
  - `changed` says whether the project's revision moved. That includes track settings, bins and marks, not only media, clips and markers.

**Kinds of request:**
- **Project requests** go through the project's `Editor`, exactly as headless.
- **Live-only requests** act on the session: `viewer.*`, `player.*`, `selection.*`, `gui.*`, `jobs.*`, `mixer.preview`, `pool.set` and `project.open` (`Request::is_live`). Run headless, they fail with `NO_INSTANCE`.
- **Slow requests** run as tasks: `export.*`, `frame.render`, `storyboard.render`, `analyze.*`, and project-less ones such as `encoders.list`. Media exports are jobs (§9).
  - A task works on a copy of the project taken when the request arrives, after its `if_rev` check, so later edits don't reach it. (Reading the saved file when the task starts could give a newer revision than the one checked.)

**Requests that wait** don't block the session. They go on a wait list:
- `player.seek` and `player.step` wait until their frame is on screen (§4).
- `gui.screenshot` waits for the picture.
- `jobs.wait` waits for the job (§9).
- Any wait gives up after 10 s (`jobs.wait` only with a `timeout`).

## 3. The project file stays the source of truth (deviation X1)

- **Saving.** Every committed change is saved before the reply, under the same lock headless runs take.
- **Locking.** The lock is retried every 5 ms.
- **Watching.** The session watches the project's directory (`notify`, non-recursive, ignoring access events). Changes by other processes are read back under the lock, so the project file and its history sidecar are always read together.
- **Write order.** `Store::save` writes the history sidecar before the project file, so a new project state always comes with its history. Otherwise a headless edit would look like an outside change and lose its undo label.
- **Sidecar failure.** If the sidecar can't be written, the edit is saved anyway:
  - The result's `warnings` says so (stderr without `--json`; a GUI also shows a notice), whether or not the command ran in a GUI.
  - The old sidecar stays whole. Another process sees the edit as one outside change, or starts with no history if there was none.
  - The GUI that made the edit keeps its undo history in memory.

## 4. When a frame is "on screen"

A seek, step, load or edit reload is a numbered **motion** of the player. It is on screen once the renderer has presented the player after the motion and shown the frame asked for:
1. **The engine has the picture.** The decode thread calls back when the picture of the viewer's frame lands, and the session asks the renderer (the viewer thread, in the GUI) to present. A kept picture is there at once.
2. **The renderer presents it** (`Player::present`), draws the picture and answers with the exact frame it showed.
   - Paused, the motion is on screen when that is the frame asked for.
   - Playing, any frame counts: the clock starts at the frame asked for and moves on.
   - A present that showed an older picture, or got no answer within 50 ms, is asked again.

**A seek also waits for the newest motion**, because an edit's reload replaces what was asked for (at the same frame). So seek-then-screenshot is exact without sleeps. A picture that fails to decode fails the seek with `MEDIA` and the reason, and the viewer shows it.

**Overtaken seeks fail with `CONFLICT`.** A waiting seek, step or viewer change remembers its frame. If a later seek asks for another frame before that one shows (another client, someone scrubbing), it never will. The request fails and names the newer frame, instead of reporting that frame as its own.
- Every seek is exact. `fast` only marks a scrub, to keep it out of the command log, so a scrub to the same frame satisfies a waiting seek.
- Playback moving on from the frame doesn't count: a seek while playing returns once its frame has shown.
- An edit's reload keeps a waiting seek's target.
- The GUI's own overtaken seeks and steps are not errors. They end as done, and the command log shows no error.

**Held keys.** A precise seek decodes from the keyframe before its frame. On files with keyframes seconds apart, that takes longer than a held arrow key takes per step. The engine merges such requests (player.md §2), so the picture moves at the speed frames decode and the last request lands exactly.

## 5. Rendering and screenshots

- **Rendering** (preview.md §4). The viewer thread draws the player's picture into a target, and the UI shows the target.
- **Hidden window.** It doesn't block seeks, viewer screenshots or edits (preview.md §2). Window screenshots and opening windows wait for the UI.
- **`gui.screenshot` targets:**
  - `viewer` (the default) reads back the player's target: the picture exactly as the viewer shows it, at the viewer's size. It works while the window is hidden.
  - `window` takes egui's viewport screenshot of the whole window. It needs the window to paint.
- **Encoding.** PNG encoding runs on a task thread.
- **Aspect.** The viewer always shows the timeline's aspect. Sources are fitted inside it the way export fits them.

## 6. Live control: registry, socket, routing

**Registry.** A running GUI writes `instances/<pid>.json` (pid, socket, open project, version) in the runtime directory: `$XDG_RUNTIME_DIR/vidcut`, else a private `vidcut-<uid>` in the temp directory, and `%TEMP%\vidcut` on Windows. It serves JSON-RPC 2.0 on `instances/<pid>.sock`, one JSON object per line (format in `ipc/mod.rs`).
- **Access.** The directory is mode 0700, so only the user can connect.
- **Windows** (`ipc::transport`): a named pipe, named after the socket's path, stands in for the socket.
  - Its access list grants the user alone. Nobody else can even read it (the default lets everyone read).
  - Every user can see pipe names, so the name holds a random number and nobody can create it first.
  - A client lets the GUI identify it, but not act as it.
  - Reads and writes are overlapped calls that a shutdown stops, which is how a cancelled client stops waiting. (On Windows, shutting down a Unix socket doesn't end a read already waiting.)
  - A pipe can't be shut down one way only, so a client that stops writing has left.
- **Limits** (`ipc::server::Limits`), so a runaway client can't take the session's memory or threads:
  - a request line of up to 64 MiB (a longer one gets an error, and the connection closes);
  - 64 connections at once (more are turned away with an error). A waiting request such as `jobs.wait` holds its connection meanwhile.
- **Clean-up.** Stale entries are removed when they're found.

**Routing.** The CLI routes each command (`ipc::route`):
- **Project-less commands** (`schema`, `probe`, `doctor`, `presets`, `encoders`, `preferences`, `project create`; `Request::is_projectless`) always run in the CLI process.
- **`--headless`** runs in the CLI process.
- **`--instance PID|SOCKET`** (or `VIDCUT_SOCKET`) picks a GUI.
- **With `-p FILE`:**
  - the GUI with that project open, if any;
  - otherwise headless, except live-only commands, which fail with `NO_INSTANCE`.
- **Without `-p`:** the only GUI with a project open. With several, it's a usage error.
- **Safety check.** Requests carry the project path, and a GUI with a different project open refuses them (`CONFLICT`).

**Paths.** The GUI may run in another directory, so the CLI makes path arguments absolute before routing, including those in `apply` batches.

## 7. Previews

- **One player.** Source Clip, Source Tape and the timeline each keep a playhead. One engine plays whichever the viewer shows, from the export's plans, proxies included (preview.md §1). The source viewer counts project frames (deviation X8).
- **Source Tape span marks** (spec D17). The scroll bar shows a one-clip span as that clip's handles offset onto the tape, and a span across clips as one shaded range. With a one-clip span, Alt+X sends `tape.marks.clear` with the clip under the playhead.
- **Reloads** after edits are coalesced: one at once, then at most one per 100 ms while edits keep coming.
  - Every live request (state, screenshots, playback, viewer changes) applies a pending reload first, so it sees the edited timeline. A seek or step loads straight at its target.
  - A reload gives the engine the new plan (`set_plan`, player.md §3).
  - While the source is shown, the timeline's new frame is prepared instead.
- **Stills and image sequences** play as export reads them. A still has one picture. A sequence has exactly its own count of pictures, even when more numbered files follow in the folder: pool thumbnails and picture measurements stay within that count, and a thumbnail time past the last picture shows the last one.
- **Importing numbered images.** Import Media (Ctrl+I, `gui/import.rs`) looks for numbered runs among the files picked and counts each run on disk. If it finds any, it asks once:
  - **One clip each, at a frame rate:** an image sequence per run (`media.add sequence`, spec D39), at the project's rate unless typed over. Offered first when a run is of a frame type (PNG, TIFF, EXR, DPX, TGA, BMP).
  - **Separate stills** (deviation X17): every file a still. Offered first for numbered JPEGs, which are usually photos.
  - The import stays one request: `media.add`, or an `apply` of two when sequences come with other files.
- **Shuttle.** J/K/L: player.md §4.

## 8. The timeline

**Two timelines** (spec §5):
- **Upper:** the whole program at full width.
- **Lower:** a detail view at a zoom you choose (Ctrl+wheel, −/+). Its playhead stays in the middle unless Timeline Options says otherwise.
- **Both draw the same clips:** names, durations, status, filmstrips (§11), and waveforms from per-media peaks (100 per second, cached in `~/.cache/vidcut/waveforms/`).

**Gestures are pure functions** (`gui/timeline/geometry.rs`, `gesture.rs`, unit-tested there):
- What the pointer is over picks the operation: edge, edit center, slip handle, audio edit, body.
- A click on an audio edit selects that J/L handle. `,` and `.` then send `edit.audio_roll` for its offset (spec D20).
- While dragging, a `Preview` says what the release would do. The timeline draws it: the target, the moved block, the snap line, a tooltip.
- The release sends that one request. Scrubs only seek.
- **Clock.** Gesture timing (the 0.8 s hover, Safe Edit's 0.6 s) uses egui's clock, so tests can drive it.

**The Trim Editor** draws exact frames rendered by the export engine on a background thread (`gui/stills.rs`).

**Tracks** (spec §5, D18, D43):
- **Rows.** Both timelines draw every track: V*n*…V1, then A1…. The upper timeline grows a little per track. The lower one shares its height among the rows, Track 1 getting the most.
- **Header column.** Its boxes (`timeline_boxes`) hold the timeline menus, the target-track selector and the toggles (Ripple On, Dynamic Trim, Split, Add Marker, Snap). The row above the timelines (`timeline_toolbar`) shows the In/Out marks, the zoom, and the clip count and length.
- **Icons** are Phosphor's (`egui-phosphor`). Each has its name as its accessible name, which tests find it by, and shows its name and key on hover.
- **Track headers** (lower timeline): enlarge at the left; the target-track toggle (the track's name, whose menu adds and removes tracks); lock, audio and video at the right, in the same columns on every row.
- **Rows sit on the bottom.** The lower timeline stacks its rows upward from a fixed strip above its bottom edge, so the bottom row stays put whatever the panel's height. Many tracks shrink to fit.
- **Destination row.** A drag's destination is the row under the pointer, or a new track in the strips above and below the rows (D18). `gesture.rs` turns it into the request's `track` (D25).

**Mixer and meters** (`gui/mixer.rs`):
- **Strips.** The Mixer panel (top bar, or `gui.show mixer`) has a strip per track with sound: one meter bar per channel, a fader, M and S. The mix gets the same per-channel meter.
- **One request per fader drag.** The fader sends `track.set` once, on release, so a drag is one undo step. While dragging, `mixer.preview` makes the level heard. Double-click resets it to 0 dB.
- **Meters** have one bar per project channel (spec D28), each with a 1.5 s peak hold.
  - They read the cached waveforms at the playhead. So they need no tap of the player's output, work paused and while scrubbing, and apply the same gains, mutes and solos as the export.
  - The export's resampler mixes the waveforms' channel envelopes to the project's channels, so waveforms are measured again when that count changes.
- **Viewer meter.** A meter beside the viewer shows what it plays, one bar per channel. Its speaker button is `player.mute`, which also silences that meter. In source modes it meters the source media at the source playhead.

**Bins:**
- **Pool view.** The pool shows the open bin, sorted (`pool.set`: bin, sort, direction). That view lives in the session, not the project, so Source Tape and agents see the same order. It follows a renamed bin, and moves to the parent bin when its own is removed.
- **Drops.** The pool records each frame where bins are drawn (a folder, or a level of the path), and the drag's release checks those spots. Which drops move is unit-tested in `gui/pool.rs`.
  - A clip drop sends `media.move`.
  - A bin drop sends one `bins.rename`, from the dragged bin to `<target>/<name>`. Nothing is sent when the target is the bin itself, a bin inside it, or its current parent.
  - If the name is taken, `bins.rename` fails and the status bar shows the error, as for other pool actions.
- **One undo step.** Create Bin With Selected Clips is one `apply` batch (`bins.add` and `media.move`).

## 9. Export windows and jobs

**Jobs.** Every media export and proxy encode in a running GUI is a job (`live/jobs.rs`), whoever started it:
- **A thread per job** prepares the export from a copy of the project (§2), reports its files and their lengths (`JobPlanned`), then encodes, sending progress about ten times a second. The session keeps each job's state for `jobs.*` and the snapshot.
- **Foreground** (an IPC client without `background`): progress notifications carry the job id, and the reply comes when the job ends. Closing the connection cancels the job. It also cancels a `frame.render`, a storyboard, an analysis, or the loudness measuring before an export, which send no progress.
- **Background** (`background: true`, and every GUI export): the reply comes once the files are planned, so mistakes (an unknown preset, an existing file) still fail the request. The job then goes on without the client. Only `jobs.cancel` cancels it.
- **`jobs.wait`** is a wait-list entry like a seek's (§2), without the 10 s limit unless given a `timeout`. It fails the way the job failed.
- **`jobs.cancel`** sets the job's cancel flag, and the engine removes partial files.
- **Kept:** running jobs and the 50 most recent finished ones.
- **`.otio`** exports write a small file at once and aren't jobs. `.otioz` and `.otiod` bundles copy whole media files, so they are jobs.

**CPU priority.** Every thread the app starts goes through `threads::spawn` and says what its work is (`Work`):
- **`Work::Job`**: exports, proxies, and other work that decodes whole files (the measuring before Detect Scene Cuts and Ripple Delete Silence, `analyze.*`, storyboards, thumbnails, waveforms, the timeline's tiles). These run at the lowest CPU priority, so the GUI, playback and other programs come first, and a job still takes every core they leave idle.
  - Linux: the idle scheduling class, which the FFmpeg threads a job starts inherit.
  - Windows: the idle priority. Windows threads don't inherit it, so FFmpeg's threads run at normal priority.
  - macOS: the utility QoS class, not inherited either. Not the background class below it, which keeps to the efficiency cores and slows disk access.
  - Not nice 19: when opening NVENC, NVIDIA's driver copies the calling thread's nice value onto the main thread, and an unprivileged process can't raise it back.
- **`Work::Interactive`** keeps normal priority: the session, requests, file dialogs, frames and plans (someone waits for them).
- **Headless runs** keep normal priority: a terminal waits for them.

**Closing.** Quitting the window while jobs run asks first (`gui.quit` doesn't). On shutdown the session cancels running jobs and waits up to 3 s for them to clean up.
- A job still running after that is stuck in a decoder or an encoder. The session removes its partial files itself, and the process ends without it.
- A job plans its files (`JobPlanned`) before writing them and starts none once cancelled. So the only file that can appear afterwards is one whose creation began before the cancel and is still blocked (on a hung network share, say).
- A client waiting for such a job, or for its plan, is told the GUI closed.

**Proxy encodes** (`proxy.generate`, spec D41) are jobs too (`kind: "proxy"`, titled by their media), so the person at the GUI sees progress and can cancel.
- The encode runs on a copy of the project. Then the request runs in the session and records the files.
- A client gets progress and is answered when the job ends. Leaving cancels it, as for a foreground export.
- The GUI's own request is logged when the job ends, with the change it made.
- Each proxy file reports its own frames, so the job's progress is that of the file being written.
- **Automatic proxies** (spec D41) are requested by the session itself (origin `session`). The Jobs window marks them automatic, and a notice shows when they end.

**The windows only send requests:**
- **Quick Export** (spec §10) lists the presets marked `quick_export`. Export… asks for a file name, starts a background export and opens its progress window.
- **The Export dialog** always starts from a preset. What the user changes becomes `--set` overrides of that preset (a JSON diff of the settings), so every GUI export is `vidcut export start OUT --preset NAME --set …`. The dialog shows that command.
- **The plan** is `export.plan`, asked again whenever the params change (one request in flight). Mistakes the dialog can see itself (codec vs container) aren't sent. Plans never change anything: per-clip folders are made when the export runs.
- **Asking the session.** Windows that need answers (presets, plans, starting an export, imports) call `Handle::call` on a helper thread and poll the result each frame (`Ask`), so the UI never waits. These calls are left out of the command log, and their errors show in the window, not the status bar.
- **File dialogs** run on helper threads behind a `Dialogs` trait. Tests answer them directly.
- **Placement.** Windows open near the middle and stay anchored at their top-left corner, so they don't move while their content changes size.
- **Project format** (spec D37). New Project asks for the format (or lets the first video set it) and the file name, then sends one `project.open` with `create`. File > Project Settings sends one `project.set` with what changed.
- **Timeline interchange** (deviation X9):
  - File > Import Timeline takes `.otio` or `.otioz` (an `.otiod` through its `content.otio`). It asks Append or Replace when the timeline has clips, and asks for a folder when media aren't found. Import warnings show in a window.
  - An `.otioz` import unpacks its media on a worker first (`Step::Measure`), so the session keeps answering meanwhile.
  - File > Export Timeline writes `.otio`. File > Export Timeline with Media writes an `.otioz`, as a job.
- **`gui.show`** opens any of these windows (or the log, or the Mixer) for the person at the GUI, for example after an agent prepared an export.

## 10. Automatic tools

- **Ripple Delete Silence.** The dialog (`gui/timeline/tools.rs`) dry-runs `edit.ripple_delete_silence` on a helper thread whenever its settings change. While open, it draws the parts as red stripes on both timelines. Remove sends the request once.
- **Measuring first.** The session measures media on a worker before such an edit runs (agents.md §3), so the GUI stays live while a long clip decodes.
- **Boring Detector.** Its highlights are computed each frame from the project (`analysis::boring`, which is cheap). Its settings are in Timeline Options, kept between runs.
- **Dynamic Trim:**
  - W, J/K/L, Space and Up/Down are handled in `dynamic_keys`. The transport keys don't play while it is on.
  - A run counts frames on egui's clock, so tests can drive it. Each frame it tries the next count on a copy of the project, and stops where an edit would fail.
  - The viewer shows the Trim Editor's two-up (`two_up`), rendered from that copy.
- **Testing menus** (`tests/gui_tools.rs`): a freshly opened menu ignores input in its first frame, so tests step twice before clicking an item.

## 11. Pool thumbnails and preferences

**Pool filmstrips** are cached on disk (`thumbcache.rs`): 24 pictures of a media file, fitted in 192×108 and spread evenly, stored as one JPEG strip in `~/.cache/vidcut/thumbs`.
- Each picture is within half the spacing of its time, so keyframes are enough for long clips. Short clips are decoded forward.
- The file name holds the media file's path hash, size and modification time, and the strip's layout. So a changed file or another layout never reads a stale strip.
- **Why on disk:** decoding the pictures is slow, especially from a network share, and would otherwise run again for every clip each time a project opens. The project still holds no pictures (deviation X1).
- **Eviction.** A strip's modification time is its last use. After each new strip, the least recently used go until the rest fit `thumbnails.cache_limit_mb` (default 512 MB).

**Preferences** (`preferences.toml`, deviation X12) are read whenever a thumbnail job, an export or a proxy runs, so a change from the CLI applies to a running GUI at once. The session also watches the preferences file, so a change from anywhere (the Preferences window, a CLI run, a hand edit) reaches the player at once:
- **GPU acceleration and GPU decoding:** the engine switches its decoder and decodes its pictures anew, the one shown first (preview.md §3).
- **The audio device:** the engine opens anew on the device chosen, paused where it was (player.md §6).
- **The player's memory** (`player.memory_mb`, `player.video_memory_mb`): over a smaller limit, the least recently kept pictures go at once.
- **The thumbnail cache:** `preferences.set` empties it when it's turned off, and trims it when the limit drops.

**Timeline filmstrips** (`gui/tiles.rs`): each tile shows the frame under it.
- **Why not the pool's pictures:** 24 per file are too sparse. A short clip would show one picture over and over, often from another shot.
- **Keys.** A tile asks for its media, source frame and height in pixels (rounded up to a multiple of 16).
  - The first tile shows the clip's first frame. The others show the first multiple of a step at or after their left edge, where the step is the largest power of two frames no wider than a tile.
  - So zooming in or out by two keeps half of the frames.
- **Memory.** Pictures are textures, kept up to `thumbnails.timeline_memory_mb` (default 256 MB). Above it, those shown least recently go, down to nine tenths, but never one shown in the last frame.
- **Making them.** Four worker threads, at job priority (§9), take the tiles that painting asked for, in its order. Each frame's asks replace the queue, so tiles scrolled away aren't made.
  - A worker goes on along its file while the next tile asked for is within 4 s ahead (decoding on beats a seek). A fresh pick avoids what another worker is near.
  - Frames come from `vidcut_av::export::renderer::FrameReader`: the export's sampling at the tile's size, from a fresh proxy when there is one.
- **On the GPU.** While `gpu.acceleration` and `gpu.decoding` are on (read as each tile is decoded, so a change applies at once):
  - HEVC, AV1 and VP9 decode on the GPU the viewer decodes on, on a stream or device of their own, and shrink there to the tile's size. Interlaced frames from VA-API and Direct3D 11 come down whole, for the CPU to deinterlace.
  - H.264 stays on the CPU: the CPU decodes it cheaply, and every seek recreates NVDEC's H.264 decoder (gotchas.md).
  - A small integrated GPU beside a fast CPU can decode tiles slower than the CPU; `gpu.decoding` off then decodes them on the CPU.
- **Disk.** Tiles go to the thumbnail cache in blocks of 64 source frames per file (`<media>.t<height>-b<block>.thumbs`), under the same limit and switch as the pool's pictures. Reading a tile from disk is far cheaper than decoding it, so a reopened project fills its tiles from disk.
- **Until a tile's picture comes,** it shows the nearest kept picture of the same media within two steps, else the pool's picture nearest in time.
