# Deviations from the Resolve Cut page

vidcut follows the Cut page ([cut-page-spec.md](cut-page-spec.md)) unless an entry here says otherwise. Every intentional difference needs an entry with:
- what Resolve does, with the manual page
- what vidcut does
- why our way is better for this tool

Additions that don't change Cut-page behavior (for example, extra scripting commands) are listed too, so the GUI-vs-Resolve picture stays complete.

Settling something the manual leaves open is a decision, not a deviation; decisions go in §12 of the spec.

## X1. Projects are always-saved standalone JSON files
- **Resolve:** projects live in a project database and are saved explicitly; Live Save is an option (manual, Project Manager chapters).
- **vidcut:** a project is one `.vidcut` JSON file, written atomically after every committed change. There is no "unsaved changes" state.
- **Why:** agents and scripts edit projects headless through the CLI, sometimes while the GUI has the same project open. One always-current file means the GUI, the CLI and the user never disagree about what the project contains. JSON is also diffable and readable for agents.

## X2. One timeline per project file
- **Resolve:** a project holds many timelines, listed in the Master bin (p. 655, 678).
- **vidcut:** a project file holds exactly one timeline.
- **Why:** simplicity for a cutting tool, and an obvious mapping for scripts. Several timelines means several project files that share media.

## X3. Export dialog beyond Quick Export
- **Resolve:** the Cut page has Quick Export only, writing a single file. Full render settings and "Individual clips" live on the Deliver page (Ch. 39, pp. 788–790).
- **vidcut:** Quick Export behaves as in Resolve. An additional Export dialog provides:
  - full codec and container settings
  - GPU encoder choice
  - per-clip export
  - audio-only export
- **Why:** vidcut has no Deliver page, and the user requires these export options.

## X4. Undo history persists across sessions and CLI runs
- **Resolve:** undo is unlimited within a session and cleared when the project closes (p. 651).
- **vidcut:** snapshot history is stored in a sidecar file under `~/.local/state/vidcut/history/`. It is capped, and survives restarts and separate CLI invocations.
- **Why:** an agent can undo its own edits from a later CLI call, and a human can undo an agent's batch from the GUI.

## X5. Scripting additions (no Cut-page behavior changes)
- **Exact-frame insert and overwrite:** `edit.insert --at` and `edit.overwrite --at` are API-only. The Cut page's Smart Insert can only insert at an existing edit point (p. 697). Scripts need deterministic, frame-exact placement that doesn't depend on a playhead.
- **Command log panel:** shows every action as its equivalent CLI command and teaches the API.

## X6. Flat media pool in v1
- **Resolve:** nested bins and a breadcrumb (pp. 655–656).
- **vidcut v1:** one flat pool. Source Tape covers the whole pool.
- **Why:** temporary scope cut. Bins are scheduled for the multi-track milestone.
