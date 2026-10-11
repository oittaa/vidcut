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
- **Resolve:** the Cut page has Quick Export only, writing a single file. Full render settings and "Individual clips" live on the Deliver page (Ch. 39, pp. 788–790), and finished renders are tracked in the Deliver page's Render Queue.
- **vidcut:** Quick Export behaves as in Resolve: a row of presets, Export, a file dialog, then a progress window (Cancel, or Hide to keep it running). The preset tiles show the preset's name and format rather than brand logos. An additional Export dialog provides:
  - full codec and container settings, starting from any preset
  - GPU encoder choice
  - range (whole timeline, In–Out, or typed), per-clip export with file-name templates, audio-only export
  - a live plan (`export.plan`: the encoder that will run, the files and their lengths) before anything is written
  - the same export as a shell command, ready to copy
  - saving the settings as a user preset, optionally added to Quick Export (p. 790)

  Every export is a job: a Jobs window lists them with progress, ETA and Cancel, including exports started from the CLI. Quitting while exports run asks first.
- **Why:** vidcut has no Deliver page, and the user requires these export options. The plan and the command make every GUI export reproducible from a script.

## X4. Undo history persists across sessions and CLI runs
- **Resolve:** undo is unlimited within a session and cleared when the project closes (p. 651).
- **vidcut:** snapshot history is stored in a sidecar file under `~/.local/state/vidcut/history/`. It is capped, and survives restarts and separate CLI invocations.
- **Why:** an agent can undo its own edits from a later CLI call, and a human can undo an agent's batch from the GUI.

## X5. Scripting additions (no Cut-page behavior changes)
- **Exact-frame insert and overwrite:** `edit.insert --at` and `edit.overwrite --at` are API-only. The Cut page's Smart Insert can only insert at an existing edit point (p. 697). Scripts need deterministic, frame-exact placement that doesn't depend on a playhead.
- **Command log panel:** shows every action as the request it sent, from any client (GUI or agent). A click copies it as a `vidcut rpc` command, which teaches the API.
- **Live control of the running GUI** ([decisions/gui.md](decisions/gui.md)):
  - CLI commands for an open project run inside the GUI, so humans and agents edit the same session.
  - Live-only commands drive it: `seek`, `play`, `viewer`, `select`, `screenshot` (the viewer picture, or the whole window), `window`, `log`, `pool` (the bin and sort order the pool shows, which Source Tape follows) and `mute` (the viewer's monitoring mute).
- **Clipboard as data:** `edit.copy` returns the clips as JSON, and `edit.paste --clips` pastes such JSON into any project. In the GUI, Ctrl+C also puts that JSON on the system clipboard.
- **API parameters for scripted edits:** `edit.overwrite`/`edit.ripple_overwrite` take `use_marks: false` to place by position (what a drag and drop does), `edit.change_duration` takes `end`, and `track.set`/`markers.set` cover the track header buttons, the mixer and the marker dialog. Edit functions and `edit.move` take `--track`, which may name the next new track (decision D27).
- **Persistent source marks:** a media item's In/Out (`media.marks.set`) are saved in the project and used by edits without a range. Resolve remembers per-clip marks too (decision D6), and this makes them scriptable.
- **Export jobs** (M5): `export --background` returns a job id at once in a running GUI, and `jobs list|get|wait|cancel` follow it. Foreground exports report progress and cancel when the client disconnects. `export.plan` (`export --dry-run`) describes an export without writing anything.
- **User presets from scripts:** `presets save` (from settings or from another preset with `--set` overrides) and `presets delete`.
- **`gui.show`** opens Quick Export, the Export dialog, Jobs, the command log or the Mixer for the person at the GUI.

## X6. Removing a bin keeps its clips
- **Resolve:** Delete Bin deletes the bin, the bins inside it and their clips (p. 377).
- **vidcut:** `bins.remove` (Remove Bin) removes the bin and the bins inside it. Their media move up into the removed bin's parent, and the timeline is untouched. Taking media out of the project stays `media.remove`, which needs `force` when timeline clips use them.
- **Why:** bins only organize the pool. A bin removed by a script or by mistake must not take media, and the timeline clips cut from them, out of the project. Undo would restore them, but an agent tidying bins shouldn't have to know that.

## X7. Timelines start at 00:00:00:00
- **Resolve:** new timelines start at record timecode 01:00:00:00 (p. 678).
- **vidcut:** the start timecode defaults to 00:00:00:00, and `project.set --start-tc 01:00:00:00` changes it. Timecodes typed into commands are record timecodes, so they follow the setting.
- **Why:** with a zero start, timecode, seconds and frame numbers all count from the same point. Scripts and agents mix all three, and a hidden one-hour offset would make `00:01:00:00` and `60s` mean different frames.

## X8. The source viewer counts project frames
- **Resolve:** Source Clip and Source Tape show a clip at its own frame rate, with its own source timecode (p. 680, 688).
- **vidcut:** source viewers step through *project* frames; for media at the project rate this is the same thing. Source times, marks and the timecode shown count project frames from the clip's first frame.
  - A 23.976 clip in a 30 fps project steps the way it will appear in the timeline: nearest-frame sampling, as export does.
  - Embedded start timecodes are not shown (yet).
- **Why:** vidcut stores every position, including `source_in` and source marks, in project frames ([cut-page-spec.md](cut-page-spec.md), time model). One unit everywhere means what you mark is exactly what lands in the timeline and in the export. The source viewer plays through the same EDL code as the timeline, so the preview matches export frame for frame.

## X9. OpenTimelineIO interchange
- **Resolve:** timelines move between applications as EDL, AAF, XML or OTIO files, imported with File > Import Timeline and exported from File > Export (pp. 527–530, 4159–4160). An import creates a new timeline, can apply the file's frame rate to the project, and asks for a folder (searched with its subfolders) when media isn't where the file says. None of this is on the Cut page.
- **vidcut:** OpenTimelineIO only, both ways (`otio.rs`):
  - `vidcut export cut.otio` (or File > Export Timeline) writes the timeline: a video track per video track; the audio of each video track's clips as an audio track (J/L cuts become audio clips offset from the video edits); a track per audio track; markers; and in `metadata.vidcut`, the In/Out marks, clip gain/mute and track settings (lock, mute, solo, fader, name). A track's video off or audio muted also marks the OTIO track disabled. Media are referenced by absolute `file://` URL. Unlike media exports it covers the whole timeline unless given a range.
  - `vidcut import cut.otio` (or File > Import Timeline) adds the media to the pool. Video tracks become V1, V2, …, each with the audio that belongs to its clips; other audio tracks become A1, A2, … (vidcut's own files map back exactly). One project holds one timeline (X2), so the import goes after the last clip, or replaces the timeline with `--replace`; the GUI asks which. An empty timeline takes the file's frame rate, start timecode and track settings. Media that moved are found by name with `--search FOLDER` (subfolders too), and the GUI asks for that folder.
  - Accepts what Resolve writes: `Clip.1`, source ranges in media timecode, float rates (snapped to exact rationals such as 24000/1001).
  - Left out, with a warning each: transitions (cuts instead), nested stacks, generators, and clips that would overlap others on their track. `.otioz` bundles aren't read.
  - Export → import reproduces a timeline frame for frame (`tests/cli.rs`, `otio.rs` tests).
- **Why:** the user's choice. OTIO is an open standard that Resolve and other editors read and write, so a cut can move to a finishing tool and back, and agents can write timelines in a documented format instead of vidcut's own.
