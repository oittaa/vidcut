# Requests

- Every action is one `Request`, from a key press in the GUI to the tool call of an agent.
- This page follows a request from its front door to `Editor::apply` and back.
- The vocabulary and the editor are in vidcut-core. The GUI's side is in [live](live.md).

```mermaid
flowchart TD
  cli["vidcut CLI<br>Cli::action"] -- "Envelope" --> run["ipc::run"]
  mcp["vidcut mcp<br>vidcut-mcp-call"] -- "Envelope" --> run
  run --> route{"ipc::route"}
  reg["ipc::registry<br>running GUIs"] -. "Instance list" .-> route
  route -- "Route::Headless" --> headless["session::run_with"]
  route -- "Route::Instance" --> client["ipc::client::call"]
  client -- "one JSON-RPC line" --> server["ipc::server<br>vidcut-ipc-conn"]
  server -- "Handle::call" --> core["Live::request<br>vidcut-session"]
  gui["GUI window<br>Workspace::send"] -- "Msg::Request" --> core
  headless -- "mutation" --> transact["Store::transact<br>lock, load, save"]
  core -- "Live::edit" --> open["OpenProject::apply<br>lock, refresh, save"]
  transact --> editor["Editor::apply"]
  headless -- "query" --> editor
  open --> editor
  headless -- "export, frame, analyze" --> render["session::render<br>on a copy"]
  core -- "Live::project_task" --> render
```

## One vocabulary

- `Request` has one variant per method. It is serialized as `method` and `params`.
- Its JSON schema (`request::protocol_schema`) generates the CLI commands (`cli::command`) and the MCP tools (`mcp::tools`). Thus every door uses the same names.
- An `Envelope` adds the two options that every request takes:
  - `if_rev`: fail with `ErrorCode::Conflict` unless the project is at that revision.
  - `dry_run`.
- `Envelope::from_json` takes these options from the request object first. It takes them from `params` only when they are not on the request object.
- Every answer is a `Response`. It holds:
  - the revision after the request
  - the ids created, modified and deleted (`Changes`)
  - the result
  - `dry_run` and `warnings`
  - where the request ran (`via`)

Three questions drive the routing:

| Question | True for |
|---|---|
| `Request::is_projectless` | needs no project: `schema`, `doctor`, `reset`, `probe`, `project.create`, presets, looks, encoders, decoders, preferences, keys |
| `Request::is_live` | needs a running GUI: `project.open`, the viewer, player, mixer preview, pool view, selection, `gui.*` and `jobs.*` |
| `Request::is_mutation` | anything that is not live and not a read, render, export, analysis, preset, preference, `project.create`, `probe` or `edit.copy`. The match in `Request::is_mutation` is the list. Undo and redo count, and call `step_history` |

## Front doors

| Door | Entry | Hands the envelope to |
|---|---|---|
| CLI | `Cli::action` gives `Action::Run` | `ipc::run` |
| MCP | `mcp::serve` reads JSON-RPC on stdin; each tool call gets a `vidcut-mcp-call` thread | `ipc::run` |
| IPC | `ipc::server::start` in the GUI: `vidcut-ipc` accepts, one `vidcut-ipc-conn` thread per connection | `Handle::call` |
| GUI | `Workspace::send` | `Handle::request`; no reply, errors become notices |

The CLI and MCP first make paths absolute (`Request::paths_mut`), because the GUI runs in its own directory.

## Routing

`ipc::run` asks `ipc::route` where the request runs:

1. Projectless requests, and the `headless` option, run here.
2. The `instance` option picks a GUI. It is a pid or a socket. `VIDCUT_SOCKET` also sets it.
3. With a project: the GUI that has it open, else here. If it runs here, a live request fails with NO_INSTANCE (`ErrorCode::NoInstance`).
4. Without a project: the only GUI that has a project open. For a live request, any GUI.
   - If there is no such GUI: here, or NO_INSTANCE for a live request.
   - If there are several: a usage error.

The registry and the client:

- The registry (`ipc::registry`) is a folder in the user's runtime dir.
- Each GUI writes an `Instance` entry (pid, socket, open project) and listens on that socket.
- `registry::list` removes the entries of dead GUIs.
- `ipc::client::call` sends one JSON-RPC line. It reads progress notifications until the answer.
- When the connection closes, the call is cancelled.

## Headless

- `session::run_with` refuses live requests. It answers projectless requests itself. For other requests, it opens the project's `Store`.
- A mutation runs in `Store::transact`:
  1. Take the lock (`Store::lock`).
  2. Load the project and its history.
  3. `Editor::apply`.
  4. Save if the revision moved.
- A query needs no lock.
- `proxy.generate` first encodes on a copy, outside the lock (spec D41).
- `subtitles.transcribe` first transcribes on a copy.
- Both do this through `work_first`. Inside the lock, the edit of these two requests finds the work done.

## Live

- `Handle::call` and `Handle::request` put a `Msg::Request` on the channel of the `vidcut-session` thread.
- `Live::request` checks the expected project and `if_rev`.
- `Live::dispatch` answers at once, or schedules the work.
- Edits go through `Live::edit` to `OpenProject::apply`. It takes the same lock as the headless runs, and reads the file again if it changed. Then it applies the edit and saves before it answers.
- The live core publishes the new `Snapshot` before the replies go. Thus, when a client hears back, the GUI's state shows the change.

```mermaid
sequenceDiagram
  participant T as vidcut edit split (terminal)
  participant C as vidcut-ipc-conn
  participant S as vidcut-session
  participant F as .vidcut file and history
  participant U as main thread (GUI)
  T->>T: Cli::action, Envelope
  T->>T: ipc::route finds the GUI with the project
  T->>C: JSON-RPC line edit.split at 10s
  C->>S: Handle::call, Msg::Request with a reply channel
  S->>S: Live::request checks project and if_rev
  S->>S: Live::dispatch, Live::edit
  S->>F: OpenProject::apply takes Store::lock, refreshes
  S->>S: Editor::apply, mutate, ops::split
  S->>F: Store::save writes history lines, then the project
  S->>S: project_changed, Live::finish logs it
  S->>U: Live::publish new Snapshot, request repaint
  S->>C: ReplyEvent::Done(Response)
  C->>T: JSON-RPC result with rev, changes, via
  U->>U: Workspace::ui reads Handle::snapshot
  F-->>S: watcher sends Msg::FileChanged, same text, no change
```

## Requests that only read

- `export.start`, `export.plan`, `frame.render`, `storyboard.render` and the `analyze` methods do not go through the editor.
- Headless, `session::run_with` loads the project, checks `if_rev` and calls `session::render`.
- A GUI calls `session::render` from `Live::project_task` on a worker thread, with a copy of the project.
- `session::render` hands exports to `session::export_media` ([export](export.md)).
- `session::render` runs the `analyze` methods as queries on a throwaway `Editor` ([analysis](analysis.md)).
- The media export of a GUI is a job (`Live::start_export`).
- None of these requests takes the lock.

## The editor

`Editor::apply` checks `if_rev`, then:

- A query goes to `query`.
- `history.undo` and `history.redo` call `step_history`.
- A mutation runs `mutate` on a clone of the project.
  - It resolves times and clip references, and calls the edit operations in `ops` (`ops::split` for `edit.split`).
  - Carried transitions that do not fit after the edit go (`otio::transitions::prune`).
  - `validate::violations` checks the result. A violation fails the edit as a vidcut bug.
- A dry run stops here and reports the `Changes`.
- An edit that changes nothing makes no revision and no undo step.
- Else the clone becomes the project, and `rev` goes up by one.
- The history then gets an `Entry`: a label (`describe`) and the `Change`.
- The label of a request that sets fields names each field that changed (`describe::set`). [Deviation X33](../deviations.md#x33-each-undo-step-says-what-where-and-how-much) lists these requests and the exceptions.
  - The old value comes from the project before the edit. The new value comes from the params.
  - A value that the project does not have shows as "usual". A field shows no old value when the things it changes have different old values.
  - A quiet field shows only its name, and does not count toward the limit of two fields with values.
- In an `apply`, `batch` names each command against the project after the commands before it.

`apply`:

- `apply` runs a list of requests in order on one clone. It makes one revision and one undo step.
- Any error fails all the requests in the list.
- Undo, redo and `apply` cannot be in the list.
- The commands in the list cannot have `if_rev` or `dry_run`. These options go on the `apply` request.

## Undo history

Changes:

- A `Change` is a tree of what differs: replaced values (`Set`), object fields (`Fields`) and lists of things with ids (`Items`).
- Thus a step costs what it changed.
- Undo applies a change backward, and still makes a new revision. Revisions only grow.

The history file:

- The history is a `.jsonl` file in the `history` folder of the state dir. A digest of the project's path names it (`HistoryFile`).
- Each save appends lines. They end with a hash of the project text that the save wrote.
- On load (`Store::load_text`), a project file that does not match that hash was changed by something else.
- Then `Editor::record_external` makes the difference from the last saved state one undoable step, by `file`.
- vidcut never writes over a history file that is there but cannot be read. Examples:
  - `HistoryFile::read_tail` fails after a few tries.
  - The last saved project of the file cannot be rebuilt.
- Then the editor keeps the steps of the session in memory and says why (`Editor::history_trouble`: a GUI notice, the warnings of `history.get`).
- The editor appends these steps when the file reads again where it left off (`Store::save`, `Store::recover`).
- A GUI notices such writes through its file watcher (`Msg::FileChanged`, `OpenProject::reload`).
- Behavior: [spec §8.10](../cut-page-spec.md#810-undo-pp-651652), [deviation X4](../deviations.md#x4-undo-history-persists-across-sessions-and-cli-runs).

Steps:

- Each `Entry` keeps a `Step` beside its `Change`. The `Step` holds:
  - the label (`describe`)
  - who made it (the `Client` of the request)
  - when
  - when its GUI opened the project (`Editor::opened_at`)
  - the clips and frames it changed (`steps::places`, from the project before and after)
  - the commands of an `apply`
  - the request that made it (`Call`: its method and params)
- A history line without these fields loads with them empty.
- The CLI and `mcp` put the `Client` on the `Envelope` (the `client` key of a JSON-RPC request).
- A GUI gives its own `Client`, and the `Client` of its IPC callers, in `Live::request`.
- `Editor::history_steps` reads the steps without their changes (`HistoryFile::steps_now`, through one handle).
- A `StepsCache` keeps what it read. Thus a GUI reads only what a save appended. After a compaction, it reads the whole file again.
- `history.get` lists the steps, with the runs that one client made in a row (`steps::listing`, `steps::groups`).
- `history.go` undoes or redoes to a step, as one revision.

The tree of versions:

- The history is a tree.
- Each saved step gets an id. `Tail` keeps the next id.
- A file that an older vidcut wrote has no next id. A save that finds none takes the next id after the largest id in the file.
- A step can go where steps can still be redone. Then it moves those steps into a `Branch` that hangs after its place (`Tree::place_at`).
- Thus the reader of the file rebuilds the versions that it already holds.
- `history.restore` undoes or redoes to the place where a version branched. Then it writes the steps of the version again under their own ids, stamped with the time of the restore.
- When the reader sees the first id of a version come back, it takes that version out.
- The steps that the restore replaces become a version, replaced at that time.
- A version inside another version comes back with the versions that hold it, outermost first, in the same revision (`Editor::chain`, `editor::climb`).
- When the user hovers over the step of a version, the timeline draws the project as it was then (`PreviewAt`, `Past::version_at`).

Compaction:

- A compaction (`HistoryFile::compact`) leaves the steps and versions together at 90% of the limit. Thus the next steps do not compact again.
- The versions get up to a tenth of the limit: named versions first, then the newest.
- The versions that hang before the oldest kept step go.
- A compaction gives no step an id. It carries the next id over.
- It drops the names of the steps that it drops. When the start moves, it also drops the name of the start.

Names:

- A name is a `Mark` line on the id of a step (`history.mark`).
- A name is saved without a new revision (`Editor::unsaved`).
- The GUI watches the history file. Thus a name given elsewhere shows at once.

Revert:

- `history.revert` applies the change of one step backward, as a new step.
- `Change::touched` and `change::overlap` find a later step that changed the same thing. Such a step refuses the revert.
- A run of items moved together touches its whole list.
- A reorder moves only the items that it listed. Thus items added after it keep their places.
- When the change does not fit, the revert names the later step after which it stops fitting.

"Since this step":

- `steps::since` compares two projects for "since this step". It compares clips, dissolves, cross fades, markers, media, tracks, bins, In and Out marks, and settings.
- For a step still to redo, it counts what a redo to that step brings (`steps::since_step`).
- For the step of a version, it counts what a move to that step brings (`steps::since_version`, `history.get` with `branch`).
- The panel and `history.get` show the same counts.
- The kind filter and the who filter offer the values that any step has, the steps of versions too (`steps::every_step`).

The History panel:

- The History panel (`HistoryUi`) docks at the right side of the window. The config folder keeps its width.
- When the current step changes, the panel scrolls only if that row is out of view. It scrolls only as far as the row needs.
- Copy as Command copies the `Call` of a step as a `vidcut rpc` command, as the export jobs do (`vidcut_command`).
- It previews a step without a change to the project.
- `Handle::history_preview` asks the live core. `Live::preview_step` hands the step to the `vidcut-preview` thread.
- That thread makes the project as it was (`Store::project_at`), only for the newest step asked. Thus a long history never holds up the live core.
- A `Past` keeps the states that it made for the current revision: the few dozen used last (an `Lru`).
- It makes each new state from the nearest state it holds (`Editor::project_between`, which reads only the steps between).
- Thus a move of one row costs one step, however long the history is.
- The snapshot publishes the result (`HistoryPreview`).
- The timeline draws the result in place of the project, its tracks and length too. It outlines what the step changed.
- The panel lays out its rows once per history and keeps them (`HistoryUi`).
- The preview of the keyboard cursor lasts while the panel has the keys.
- While a past state is drawn, a press on the timeline only ends the preview, up to its release (`Workspace::history_swallows`).
- An undo, redo or `history.go` from the GUI publishes a `Flash` of what it moved through.
- Behavior: [deviations X33–X38](../deviations.md#x33-each-undo-step-says-what-where-and-how-much).

**Why** the lock, the revision and the reload: the CLI, agents and the GUI edit one file at the same time ([deviation X1](../deviations.md#x1-projects-are-always-saved-standalone-json-files)).

- The lock orders the writers.
- The file always holds the latest project.
- `if_rev` refuses an edit to a project that changed after the client read it.

## Code

- `crates/vidcut-core/src/request.rs`
- `crates/vidcut-core/src/editor.rs`
- `crates/vidcut-core/src/store.rs`
- `crates/vidcut-core/src/history.rs`
- `crates/vidcut-core/src/steps.rs`
- `crates/vidcut-core/src/describe.rs`
- `crates/vidcut-core/src/change.rs`
- `crates/vidcut-core/src/ops.rs`
- `crates/vidcut/src/main.rs`
- `crates/vidcut/src/cli.rs`
- `crates/vidcut/src/mcp.rs`
- `crates/vidcut/src/ipc/`
- `crates/vidcut/src/session.rs`
- `crates/vidcut/src/live/mod.rs`
- `crates/vidcut/src/live/project.rs`
- `crates/vidcut/src/gui/workspace.rs`
- `crates/vidcut/src/gui/history.rs`
