# GUI and live control: decisions from M3

**Status:** accepted (October 2026). The viewer plays through the player engine on wgpu ([preview.md](preview.md)).

**Evidence** (all rerunnable):
- [`crates/vidcut/tests/live.rs`](../../crates/vidcut/tests/live.rs): the live session with its one player engine, a stand-in renderer and a stand-in audio device, no window. Covers frame-exact seeks at every boundary (also with a late renderer), pictures against the sound, the source viewers and switching between them and the timeline (the frame kept ready, a fader held), the clipboard, edits from other processes, and errors.
- [`crates/vidcut/tests/gui.rs`](../../crates/vidcut/tests/gui.rs) (`--features vidcut/gui-tests`, needs a display): the real GUI driven through the CLI. Covers edit latency, screenshot vs `vidcut frame render`, a minimized window, playing and the GPU preference.
- `ipc::tests` in [`crates/vidcut/src/ipc/mod.rs`](../../crates/vidcut/src/ipc/mod.rs): the JSON-RPC wire format.
- [`crates/vidcut/tests/jobs.rs`](../../crates/vidcut/tests/jobs.rs) and [`gui_export.rs`](../../crates/vidcut/tests/gui_export.rs): export jobs, and the export windows under simulated input (§9).

## 1. Threads

| Thread | Owns | Never does |
|---|---|---|
| **UI** (eframe on wgpu) | the egui context and its window; shows the viewer's targets as textures | draw a viewer picture, wait for the session |
| **Viewer** (`gui/video.rs`) | presents the player's engine: draws its picture, answers presents and viewer screenshots | wait for the UI |
| **Session** (`live`) | the open project, the player's engine, viewer/selection/clipboard state, the command log | touch the GPU |
| **Engine** (player.md §1) | decoding, the mix, the audio device | touch shared state |
| **IPC** | one listener plus one thread per connection | anything but forward requests and wait for answers |
| **Workers** (2) | thumbnails and waveforms | touch shared state (they report back by message) |
| **Tasks** | one thread per export job, frame render, screenshot encode or project-less request | touch the session's state |

**How they talk:**
- **One inbox.** Everyone talks to the session through one `mpsc` inbox (`Handle`).
- **Snapshots out.** After each batch of messages, the session publishes an immutable `Arc<Snapshot>`, and the UI reads the latest one each frame. Publishing is a pointer swap under a mutex.
- **Answers after the snapshot.** The session sends a request's answer only once the snapshot shows its result. A client that looks right after hearing back (the GUI, a test stepping it) never sees the state from before. Answering first let a loaded test draw a stale selection and send nothing.
- **No blocking.** The UI therefore never blocks on the session, and the session never blocks on the UI.
- **Repaints.** The session calls `request_repaint` when it publishes or asks the UI for something (a capture of the window, a window change), and the viewer thread when a picture it drew changed. Nothing else wakes the UI. While playing, the session publishes as the clock moves to each frame.

## 2. One request path

Every action is a [`Request`](../../crates/vidcut-core/src/request.rs), the same type the headless CLI runs:
- **GUI gestures** send one request each, without waiting for an answer. Failures show as notices in the status bar.
- **IPC clients** send a request and wait for its answer.
- **The command log** shows every request with its origin (`gui`, `ipc`, `session`), so a human can see what an agent did and an agent can read what the human did (`gui.log`). Each entry has a `summary` in words (`vidcut_core::describe`): times as timecodes, media by name, and for an undo or redo the steps it took, which the undo history names the same way. The GUI shows the summary and the request on hover. Fast scrubbing seeks and a fader's drag (`mixer.preview`) are left out, and the GUI asks for the timeline view only when the viewer isn't showing it. An entry's `changed` says whether the request changed the project: its revision moved, even if no media, clip or marker did (track settings, bins, marks).

**Kinds of request:**
- **Project requests** go through the project's `Editor`, exactly as headless.
- **Live-only requests** act on the session: `viewer.*`, `player.*`, `selection.*`, `gui.*` and `project.open`. Run headless, they fail with `NO_INSTANCE`.
- **Slow requests** run as tasks: `export.*`, `frame.render`, `storyboard.render`, `analyze.*`, and project-less ones such as `encoders.list`. Media exports are jobs (§9).
  - A task works on a copy of the project taken when the request arrives, after its `if_rev` check. Edits that follow, by anyone, don't reach it. Reading the saved file when the task starts could give a newer revision than the one checked.

**Requests that wait** don't block the session. They become entries in a wait list:
- **`player.seek` and `player.step`** wait until the frame is on screen (§4).
- **`gui.screenshot`** waits for the picture.
- **Limit.** Any wait gives up after 10 s.

## 3. The project file stays the source of truth (deviation X1)

**Saving.** Every committed change is saved before the reply, under the same lock headless runs take.

**Watching.** The session watches the project's directory (`notify`, non-recursive, ignoring access events). Changes made by other processes are read back under the lock, so the project file and its history sidecar are always seen together.

**Write order.** `Store::save` writes the history sidecar before the project file. The new state then shows up with its history; otherwise a headless edit would be mistaken for an "external change" and lose its undo label.
- If the sidecar can't be written, the edit is saved all the same, and the command's JSON result says so in `warnings` (stderr without `--json`; a GUI also shows a notice), routed into the GUI or not. The old sidecar stays whole, so another run sees the edit as one outside change, or starts with no history if there was none. The GUI that made the edit keeps its undo history in memory.

**Locking.** The lock is retried every 5 ms. An edit made by another process shows up in the GUI within about 6 ms (`tests/live.rs`).

## 4. When a frame is "on screen"

A seek, step, load or an edit's reload is a **motion** of its player, numbered. It is on screen once the renderer presented the player after it and showed the frame it asked for:
1. **The engine has the picture.** Its decode thread calls back when the picture of the frame the viewer is at lands; the session then asks the renderer (the viewer thread in the GUI) to present. A picture already kept is there at once.
2. **The renderer presented it.** The renderer presents the engine at that moment (`Player::present`), draws the picture, and answers with the frame it showed exactly. Paused, the motion is on screen when that is the frame asked for; playing, when any frame showed (the clock starts at the frame asked for and moves on). A present that showed an older picture, or went unanswered for 50 ms, is asked again.

`tests/live.rs` ("A late renderer") stalls the render thread for 500 ms when a seek's frame arrives, and the seek still returns with that frame on screen.

**More waiting.** A seek also waits for the player's newest motion: an edit's reload replaces what was asked for, at the same frame. Seek-then-screenshot is therefore exact without sleeps. Over IPC it takes 2–3 ms for a kept picture, a decode's time otherwise. A picture that fails to decode fails the seek with the reason (`MEDIA`), and the viewer shows it.

**Overtaken seeks fail.** A waiting seek, step or viewer change remembers the frame it asked for. If a later seek asks for another frame before it shows (another client, a person scrubbing), the frame never will. The request then fails with `CONFLICT` and names the frame asked for since, rather than reporting the other frame as its own.
- Every seek is exact: `fast` marks a scrub, which only keeps it out of the command log, so a scrub to the same frame satisfies a waiting seek.
- Playback moving on from the frame doesn't count, so a seek while playing returns once its frame has shown.
- An edit's reload keeps the engine where it is (`set_plan`), playing on if it plays, and keeps the seek's target.
- The GUI's own overtaken seeks and steps are no errors: they end as if done, and the command log shows no error for them.

**Held keys.** A precise seek decodes from the keyframe before its frame, which takes a tenth of a second on files with keyframes seconds apart. A held arrow key steps faster than that. The engine merges requests: while a paused seek decodes, newer ones wait, and only the newest goes once it finishes, unless it has decoded for a second (player.md §2). The picture moves at the speed frames decode, and the last request lands exactly.

## 5. Rendering and screenshots

**Rendering** (preview.md §4). The viewer thread draws the player's picture; the UI shows the target. A hidden window doesn't block seeks, viewer screenshots or edits (preview.md §2); window screenshots and opening windows wait on the UI.

**`gui.screenshot` targets:**
- **`viewer`** (the default) reads the player's target back: the picture exactly as the viewer shows it, at the viewer's size. It works while the window is hidden.
- **`window`** uses egui's viewport screenshot of the whole window. It needs the window to paint.

**Encoding.** PNG encoding runs on a task thread.

**Aspect.** The viewer shows the timeline's aspect in every mode, and sources are fitted inside it the way export fits them. Render targets are sized to the fitted picture in physical pixels.

## 6. Live control: registry, socket, routing

**Registry.** A running GUI writes `$XDG_RUNTIME_DIR/vidcut/instances/<pid>.json` with its pid, socket, open project and version. It serves JSON-RPC 2.0 on `<pid>.sock`, with one JSON object per line (format in `ipc/mod.rs`).
- **Access.** The directory is mode 0700, so only the user can connect.
- **Windows** (`ipc::transport`): a named pipe stands in for the socket, named after the socket's path, and `%TEMP%\vidcut` for the runtime directory. Its access list grants the user alone, nobody else may even read it (the default lets everyone read); the address holds a random number, as pipe names are seen by every user, so none can take it first; and a client lets the GUI identify it, not act as it. Reads and writes are overlapped calls that a shutdown stops: a Unix socket on Windows doesn't end a read already waiting when another thread shuts it down, and that is how a cancelled client stops waiting. A pipe can't be shut down one way only, so a client that stops writing has left.
- **Limits** (`ipc::server::Limits`), so a runaway client can't take the session's memory or threads: a request line up to 64 MiB (a longer one gets an error and the connection closes) and 64 connections at once (more are turned away with an error). A waiting request such as `jobs.wait` keeps its connection, as intended.
- **Clean-up.** Stale entries are removed when they're found.

**Routing.** The CLI routes each command:
- **Project-less commands** (`schema`, `probe`, `presets`, `encoders`, `doctor`) always run in the CLI process.
- **`--headless`** runs in the CLI process.
- **`--instance PID|SOCKET`** (or `VIDCUT_SOCKET`) picks a GUI.
- **With `-p FILE`:**
  - the GUI with that project open, if any;
  - otherwise headless, except live-only commands, which fail with `NO_INSTANCE`.
- **Without `-p`:** the only GUI with a project open; with several GUIs, it's a usage error.
- **Safety check.** Requests carry the project path, and a GUI that has a different project open refuses them (`CONFLICT`).

**Paths.** The CLI makes path arguments absolute before routing, those in `apply` batches too, since the GUI may run in another directory. Raw `rpc` requests should use absolute paths.

## 7. Previews

**One player.** Source Clip, Source Tape and the timeline each keep a playhead. One engine plays whichever the viewer shows (preview.md §1).

**Source Tape span marks** (spec D17). The scroll bar shows a one-clip span as that clip's handles offset onto the tape, and a span across clips as one shaded range. Alt+X sends `tape.marks.clear` with the clip under the playhead when the span is one clip.

**What it loads.** The engine plays the export's plans, proxies included (preview.md §1). The source viewer counts project frames (deviation X8).

**Reloads.** Timeline reloads are coalesced: one at once, then at most one per 100 ms while edits keep coming. Every live request (state, screenshots, playback, viewer changes) applies a pending reload first, so it sees the timeline as edited; a seek or step loads straight at its target. A reload gives the engine the new plan: its position, open files and kept pictures stay, and it plays on if it plays. While the source is shown, the timeline's new frame is prepared instead.

**Stills and image sequences (M9).** The engine reads them as export does. The pool's thumbnails of a sequence spread over its own pictures — its count, even when more numbered files follow in the same folder — and picture measurements stop at that count. A still has its one picture. A thumbnail time past the last of those pictures stays on the last one and does not open the next path.

**Importing numbered images (M9).** Import Media (Ctrl+I, `gui/import.rs`) looks for numbered runs among the files picked, counting each run on disk. If there are any, it asks once:
- **One clip each, at a frame rate:** an image sequence per run (`media.add sequence`), at the project's rate unless typed over. Offered first when a run is of a frame type (PNG, TIFF, EXR, DPX, TGA, BMP).
- **Separate stills:** every file a still. Offered first for numbered JPEGs, which are usually photos.
- The import stays one request: `media.add`, or an `apply` of two when sequences come with other files.

**Shuttle.** J/K/L: player.md §4.

## 8. The timeline (M4)

**Two timelines** (spec §5):
- **Upper:** the whole program at full width.
- **Lower:** a detail view at a zoom you choose (Ctrl+wheel, −/+), its playhead fixed in the middle unless Timeline Options says otherwise.
- **Shared drawing.** Both draw the same clips: names, durations, status, filmstrips (§11), waveforms from per-media peaks (100 per second, cached in `~/.cache/vidcut/waveforms/`).

**Gestures are pure functions** (`gui/timeline/geometry.rs`, `gesture.rs`):
- What the pointer is over picks the operation: edge, edit center, slip handle, audio edit, body.
- A click on the audio edit selects that J/L handle. `,` and `.` then send `edit.audio_roll` for its offset (spec D20).
- While dragging, a `Preview` says what the release would do. The timeline draws it: the target, the moved block, the snap line, a tooltip.
- The release sends that one request. Scrubs only seek.
- **Clock.** Gesture timing (the 0.8 s hover, Safe Edit's 0.6 s) uses egui's clock, so tests drive it.

**Tests:**
- `geometry.rs` and `gesture.rs` unit tests cover zones, snapping and every drop rule.
- `tests/gui_input.rs` (egui_kittest, no window) clicks, drags and types into the real panels against a live session. It checks that each gesture changes the project with exactly one request and that the command log matches the undo history. It also times frames with ~2,000 clips: about 2.5 ms of UI work in a debug build.

**The Trim Editor** draws exact frames rendered by the export engine on a background thread (`gui/stills.rs`).

**Several tracks (M6).**
- **Rows:** both timelines draw every track: V*n*…V1, then A1…. The upper timeline grows a little per track, and the lower one shares its height among the rows (Track 1 gets the most).
- **The header column's boxes** (spec §5, as the Cut page has them): beside the overview, icons for the Timeline Options, Timeline Actions (tracks, split, marker, Ripple Delete Silence, Detect Scene Cuts) and Edit Actions (trim to playhead, clip duration) menus, and a selector of the target track for edit functions; beside the lower ruler, toggles for Ripple On, Dynamic Trim, Split, Add Marker and Snap (where the Cut page has Keyframes: vidcut has no effects). The icons are Phosphor's (`egui-phosphor`); each is read by its name (accessibility, tests) and says its name and key on hover. The row above the timelines keeps the In/Out marks, zoom and what the timeline holds.
- **Headers:** the lower timeline's track headers hold the enlarge button, the target-track toggle (the name, with a menu to add or remove tracks) and the lock, audio and video buttons, as icons (spec D43). Every row puts them on one line centered in it, each button in the same column row under row: enlarge on the left, the others at the right edge, the name in what they leave (cut short if need be).
- **Rows sit on the bottom:** the lower timeline lays its rows out upward from a fixed strip above its bottom edge (room to drop a new audio track), so V1 or the last audio track stays put whatever the panel's height; the spare room is above the top track, where a drop makes a new video track. Many tracks shrink to fit between the strips.
- **Destination row:** a drag's destination is the row under the pointer, and the strips above and below the rows stand for a new track (spec D18). `gesture.rs` turns the destination into the request (`track`, D25), and its unit tests cover the rows.

**Mixer and meters (M6, `gui/mixer.rs`).**
- **Strips:** the Mixer panel (top bar, or `gui.show mixer`) has a strip per track with sound: one meter bar per channel, a fader, M and S. The mix gets the same per-channel meter.
- **One request per fader drag:** the fader sends `track.set` once, when it's let go, so a drag is one undo step. Double-click resets it to 0 dB.
- **Meters** are one bar per project channel (spec D28), with a 1.5 s peak hold on each bar. They read the cached waveforms at the playhead, playing or not, so they need no tap of the player's output, work while paused and while scrubbing, and apply the same gains, mutes and solos as the export. The waveforms' channel envelopes are mixed to the project's channels by the export's resampler, so they are measured again when that count changes.
- **Viewer:** a meter beside the viewer shows what it plays, one bar per channel, and its speaker button is `player.mute`. That mute silences the viewer meter. In source modes the meter is the source media at the source playhead.

**Bins (M6).**
- **Pool view:** the pool shows the open bin (`pool.set`: bin, sort, direction). That view lives in the session, not the project, so Source Tape and agents see the same order. It follows a renamed bin, and moves to the parent bin when its own is removed.
- **Drops:** the pool records where bins are drawn each frame (a folder, or a level of the path) and the drag's release checks them. A clip drop sends `media.move`. A bin drop sends one `bins.rename`, from the dragged bin to `<target>/<name>`. Nothing is sent when the target is the bin itself, a bin inside it, or its current parent. If the name is taken, `bins.rename` fails and the status bar shows the error, the same as other pool actions. The open bin follows a renamed bin, as any rename does.
- **One undo step:** Create Bin With Selected Clips is one `apply` batch (`bins.add` and `media.move`). A bin drag is the one `bins.rename`.
- **Tests:** `tests/gui_tracks.rs` (egui_kittest) covers the target track, a drag onto a new track, Place On Top, a fader drag, solo, New Bin, a clip dropped into a bin, and a bin dropped into another bin. Each sends exactly one request. Which bin drops move is unit-tested in `gui/pool.rs`.

## 9. Export windows and jobs (M5)

**Jobs.** Every media export and proxy encode in a running GUI is a job (`live/jobs.rs`), whoever started it:
- **A thread per job** prepares the export from a copy of the project (§2), reports its files and their lengths (`JobPlanned`), then encodes, sending progress about ten times a second. The session keeps each job's state for `jobs.*` and the snapshot.
- **Foreground** (an IPC client without `background`): progress notifications carry the job id, the reply comes when the job ends, and a closed connection cancels it. That same closed connection cancels a `frame.render`, a storyboard, an analysis and the loudness measure before the export, which send no progress of their own. A background job is cancelled only by `jobs.cancel`.
- **Background** (`background: true`, and every GUI export): the reply comes once the files are planned, so mistakes (an unknown preset, an existing file) still fail the request itself. The job goes on without the client.
- **`jobs.wait`** is a wait-list entry like a seek's (§2), without the 10 s limit unless given a `timeout`; it fails the way the job failed. `jobs.cancel` sets the job's cancel flag, and the engine removes partial files.
- **Low priority:** every thread the app starts goes through `threads::spawn` and says what its work is. A job (`Work::Job`) drops to the lowest CPU priority as it starts, Linux's idle scheduling class, and the FFmpeg threads it starts inherit it. On Windows the job's thread takes the idle priority, and FFmpeg's threads keep the normal one: Windows threads don't inherit priorities. On macOS the job takes the utility QoS class, which threads don't inherit either; not the background class below it, which keeps to the efficiency cores and slows disk access, so a job wouldn't take the cores and the disk left idle. Not nice 19: NVIDIA's driver, opening NVENC, copies the calling thread's nice value onto the main thread, and nothing unprivileged raises it again; the idle class leaves nice values alone. The GUI, playback and other programs come first, and the job still takes every core they leave idle. So do the other requests that decode whole files: the measuring before Detect Scene Cuts and Ripple Delete Silence, `analyze.*` and storyboards. Frames and plans keep their priority: someone waits for them, and they're quick. Headless runs keep their priority: a terminal waits for them.
- **Kept:** running jobs and the 50 most recent finished ones.
- **Closing:** quitting the window while jobs run asks first (`gui.quit` doesn't). The session cancels running jobs on shutdown and waits up to 3 s for them to clean up.
  - A job still running after that is stuck in a decoder or an encoder. The session removes its partial files itself, and the process ends without it.
  - A job plans its files (`JobPlanned`) before it writes them, and once cancelled starts none. So only a file whose creation began before the cancel and is still blocked (on a hung network share, say) can appear afterwards.
  - A client waiting for such a job, or for its plan, hears that the GUI closed.
- `.otio` exports write a small file at once and aren't jobs. `.otioz` and `.otiod` bundles copy whole media files, so they are jobs like media exports.
- **Proxy encodes** (`proxy.generate`, spec D41) are jobs too (`kind: "proxy"`, titled by their media), so the person at the GUI sees progress and can cancel. The encode runs on a copy of the project; the request then runs in the session and records the files. A client is answered when the job ends, with progress on the way, and leaving cancels it, as for a foreground export. The GUI's own request is logged when the job ends, with the change it made. Each proxy file reports its own frames, so the job's progress is the file being written. Automatic proxies (spec D41) are requests the session sends itself after an import or after `use_proxies` is turned on: origin `session`, shown as automatic in the Jobs window, with a notice when they end.

**The windows only send requests:**
- **Quick Export** (p. 788) lists the presets marked `quick_export`; Export… asks for a file name, then starts a background export and opens its progress window.
- **The Export dialog** always starts from a preset. What the user changes becomes `--set` overrides of that preset (a JSON diff of the settings), so every GUI export reads as `vidcut export start OUT --preset NAME --set …`. The dialog shows that command; `tests/gui_export.rs` parses it with the real CLI parser and checks it makes the same request as the GUI, and that the plan headless equals the plan in the GUI (M5 acceptance).
- **What this machine can't export** is greyed out where it is chosen, the reason on hover: Quick Export's tiles and the dialog's presets (`presets.list`'s `unavailable`), codecs and encoders (`encoders.list`). Nothing says so before.
- **The plan** is `export.plan`, asked again whenever the params change (one request in flight). Mistakes the dialog can see itself (codec vs container) don't go to the session. Plans never change anything: per-clip folders are made when the export runs.
- **Asking the session:** windows that need answers (presets, plans, starting an export, imports) call `Handle::call` on a helper thread and poll the result each frame (`Ask`), so the UI never waits. These lookups are left out of the command log, and their errors show in the window instead of the status bar.
- **File dialogs** run on helper threads behind a `Dialogs` trait; tests answer them directly.
- **Placement:** windows open near the middle and stay anchored at their top-left, so they don't move while their content changes size.
- **Project format** (spec D37): New Project asks whether the first video sets the format or takes a size (a list, or any even width and height) and a frame rate (the common ones, or any typed rate), then the file name, and sends one `project.open` with `create` and them. File > Project Settings sends one `project.set` with what changed; its rate field is off while the timeline has clips.
- **Timeline interchange** (deviation X9): File > Import Timeline (`.otio` or `.otioz`; an `.otiod` through its `content.otio`) asks Append or Replace when the timeline has clips, and asks for a folder when media aren't found (p. 529). Import warnings show in a window. An `.otioz` import unpacks its media on a worker first (`Step::Measure`), so the session keeps answering meanwhile. File > Export Timeline writes `.otio`; File > Export Timeline with Media writes an `.otioz` as a job, as Resolve's export offers both (p. 4160).
- **`gui.show`** opens any of these windows (or the log, or the Mixer) for the person at the GUI, e.g. after an agent prepared an export.

## 10. Automatic tools (M7)

- **Ripple Delete Silence:** the dialog (`gui/timeline/tools.rs`) dry-runs `edit.ripple_delete_silence` on a helper thread whenever its settings change. It draws the parts as red stripes on both timelines while it is open. Remove sends the request once.
- **Measuring first:** the session measures media on a worker before such an edit runs (agents.md §3), so the GUI stays live while a long clip decodes.
- **Boring Detector:** its highlights are computed each frame from the project (`analysis::boring`, cheap). Its settings are Timeline Options, kept between runs.
- **Dynamic Trim:**
  - W, J/K/L, Space and Up/Down are handled in `dynamic_keys`; the transport keys don't play while it is on.
  - A run counts frames on egui's clock, so tests drive it. Each frame it tries the next count on a copy of the project, and stops where an edit would fail.
  - The viewer shows the two-up from the Trim Editor (`two_up`), rendered from that copy.
- **Tests:** `tests/gui_tools.rs` (egui_kittest) checks that each tool makes one request. A freshly opened menu ignores input in its first frame, so tests step twice before clicking an item.

## 11. Pool thumbnails and preferences

- **Filmstrips are kept on disk** (`thumbcache.rs`): 24 pictures of a media file, fitted in 192×108 and spread evenly, as one JPEG strip in `~/.cache/vidcut/thumbs` (about 6 KB a picture, where the RGBA in memory takes 83 KB). Each is within half the spacing of its time, so keyframes are enough for long clips; short clips are decoded forward. About 0.1–3 s a clip on workers. The file name holds the media file's path hash, size and modification time, and the strip's layout, so a changed file or another layout never reads a stale strip.
- **Eviction:** a strip's modification time is its last use. After each new strip, the least recently used go until the rest fit `thumbnails.cache_limit_mb` (512 MB by default, about 3,000 media files).
- **Why:** decoding 24 pictures takes from a tenth of a second to seconds per clip, much more from a network share, and it ran again for every clip each time a project opened. The project still keeps no pictures (deviation X1).
- **Preferences** (`preferences.toml`, deviation X12) are read whenever a thumbnail job, an export or a proxy runs, so a change from the CLI applies to a running GUI at once. GPU acceleration, GPU decoding and the audio device also reach the player at once: the session watches the preferences file, and when one changed, whoever changed it (the Preferences window, a CLI run, the file by hand), the engine switches its decoder and decodes its pictures anew, the one shown first (preview.md §3), or opens anew on the audio device chosen, paused where it was (player.md §6). The player's memory (`player.memory_mb`, `player.video_memory_mb`) reaches it the same way: over a smaller limit, the pictures kept least recently go at once. `preferences.set` empties the cache when it is turned off and trims it when the limit drops.
- **Tests:** `tests/thumbnails.rs` opens a project whose media moved away and still gets its thumbnails, then turns the cache off in the Preferences window with one request.

**Timeline filmstrips** (`gui/tiles.rs`): each tile shows the frame under it.
- **Why not the pool's pictures:** 24 per file is one every 30 s of a 12-minute film. A 5 s clip showed one picture over and over, often from another shot.
- **Keys:** a tile asks for its media, source frame and height in pixels (rounded up to a multiple of 16). The first tile shows the clip's first frame; the others the first multiple of a step at or after their left edge, where the step is the largest power of two frames no wider than a tile. Zooming in or out by two keeps half of the frames.
- **Memory:** pictures are textures, kept up to `thumbnails.timeline_memory_mb` (256 MB by default, about 12,000 tiles 56 px tall). Above it, those shown least recently go, down to nine tenths; never one shown in the last frame.
- **Making them:** four worker threads at the lowest CPU priority take the tiles painting asked for, in its order, and replace their queue with each frame's asks, so tiles scrolled away aren't made. A worker goes on along a file while the next tile is asked for within 4 s ahead (decoding on beats a seek), and a fresh pick avoids what another worker is near. Frames come from `vidcut_av::export::renderer::FrameReader`, the export's sampling at the tile's size, from a fresh proxy when there is one. While `gpu.acceleration` and `gpu.decoding` are on (read as each tile is decoded, so a change reaches them at once), HEVC, AV1 and VP9 decode on the GPU the viewer decodes on, on a stream or device of their own, and shrink there to the tile's size (interlaced frames of VA-API and Direct3D 11 come down whole, for the CPU to deinterlace); H.264 stays on the CPU, which decodes it cheaply, where a seek makes NVDEC's H.264 decoder anew (gotchas.md). A far-apart tile of 4K60 10-bit HEVC (the noise clip, preview.md §3) took 72–420 ms on the N150's VA-API against 0.35–2.2 s on its CPU, and 199–278 ms on NVDEC against 248–493 on the 9800X3D's CPU; the 9800X3D's small Radeon took longer than its CPU (492–675 ms).
- **Disk:** tiles go to the thumbnail cache in blocks of 64 source frames per file (`<media>.t<height>-b<block>.thumbs`), under the same limit and switch as the pool's pictures. A JPEG tile takes 2–4 KB and decodes in 0.03 ms; decoding a far-apart tile takes 40 ms on 1080p H.264 and 105–170 ms on 4K60 H.264 (20–37 ms on their AV1 proxies), so a reopened project fills its tiles from disk.
- **Until a tile's picture comes:** the nearest kept picture of the same media within two steps, else the pool's picture nearest in time.
- **Tests:** `tests/gui_tiles.rs` checks that the visible tiles are made and kept on disk, then that a new window finds them there after the media moved away. `vidcut-av/tests/frames.rs` checks the frames by barcode; the grid and the eviction are unit tests.

## 12. Still open

Open GUI work is in [docs/todo/](../todo/), one file per item.
