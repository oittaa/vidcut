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
- **vidcut:** Quick Export behaves as in Resolve: a row of presets, Export, a file dialog, then a progress window (Cancel, or Hide to keep it running). It and Export are buttons in the top bar, not also File menu items: the bar always shows them. The preset tiles show the preset's name and format rather than brand logos. The row is shorter than Resolve's and starts with the modern codecs: H.265 Master, AV1 (10-bit, at the timeline's size), ProRes 422 HQ, YouTube 1080p and 2160p, TikTok. H.264 Master, YouTube 720p and Vimeo stay available as presets in the Export dialog, which starts from H.265 Master. An additional Export dialog provides:
  - full codec and container settings, starting from any preset
  - GPU encoder choice
  - range (whole timeline, In–Out, or typed), per-clip export with file-name templates, audio-only export
  - a live plan (`export.plan`: the encoder that will run, the files and their lengths) before anything is written
  - the same export as a shell command, ready to copy
  - saving the settings as a user preset, optionally added to Quick Export (p. 790)
  - loudness normalization to a target in LUFS (`audio.loudness`), like the Deliver page's Normalize Audio (p. 4105): the mix is measured (EBU R128), then changed by one gain that keeps true peaks at or below −1 dBTP. The export reports what it measured and the gain

  Every export is a job: a Jobs window lists them with progress, ETA and Cancel, including exports started from the CLI. Quitting while exports run asks first.
- **Why:** vidcut has no Deliver page, and the user requires these export options. The plan and the command make every GUI export reproducible from a script. A short row is quicker to choose from. AV1 makes the smallest files at a given quality, YouTube and other sites take it, and current GPUs encode it in hardware as fast as H.264; H.265 has replaced H.264 for masters.

## X4. Undo history persists across sessions and CLI runs
- **Resolve:** undo is unlimited within a session and cleared when the project closes (p. 651).
- **vidcut:** snapshot history is stored in a sidecar file under `~/.local/state/vidcut/history/`. It is capped, and survives restarts and separate CLI invocations.
- **Why:** an agent can undo its own edits from a later CLI call, and a human can undo an agent's batch from the GUI.

## X5. Scripting additions (no Cut-page behavior changes)
- **Exact-frame insert and overwrite:** `edit.insert --at` and `edit.overwrite --at` are API-only. The Cut page's Smart Insert can only insert at an existing edit point (p. 697). Scripts need deterministic, frame-exact placement that doesn't depend on a playhead.
- **Command log panel:** shows every action as the request it sent, from any client (GUI or agent). A click copies it as a `vidcut rpc` command, which teaches the API.
- **Live control of the running GUI** ([decisions/gui.md](decisions/gui.md)):
  - CLI commands for an open project run inside the GUI, so humans and agents edit the same session.
  - Live-only commands drive it: `player.*` (seek, step, play, mute: the viewer's monitoring mute), `viewer.*`, `selection.*`, `gui.screenshot` (the viewer picture, or the whole window), `gui.window`, `gui.log`, and `pool.set` (the bin and sort order the pool shows, which Source Tape follows).
- **Clipboard as data:** `edit.copy` returns the clips as JSON, and `edit.paste --clips` pastes such JSON into any project. In the GUI, Ctrl+C also puts that JSON on the system clipboard.
- **API parameters for scripted edits:** `edit.overwrite`/`edit.ripple_overwrite` take `use_marks: false` to place by position (what a drag and drop does), `edit.change_duration` takes `end`, and `track.set`/`markers.set` cover the track header buttons, the mixer and the marker dialog. Edit functions and `edit.move` take `--track`, which may name the next new track (decision D27).
- **Persistent source marks:** a media item's In/Out (`media.marks.set`) are saved in the project and used by edits without a range. Resolve remembers per-clip marks too (decision D6), and this makes them scriptable.
- **Export jobs** (M5): `export --background` returns a job id at once in a running GUI, and `jobs list|get|wait|cancel` follow it. Foreground exports report progress and cancel when the client disconnects. `export.plan` (`export --dry-run`) describes an export without writing anything.
- **User presets from scripts:** `presets save` (from settings or from another preset with `--set` overrides) and `presets delete`.
- **`gui.show`** opens Quick Export, the Export dialog, Jobs, the command log or the Mixer for the person at the GUI.
- **Measuring as data** (M7): `analyze silence|scenes|black|loudness` report what Ripple Delete Silence, Detect Scene Cuts, a black check and a loudness meter find, as frame ranges, without changing anything. `vidcut timeline boring` is the Boring Detector's finding as a list. Agents use them to plan edits (markers, cuts, In/Out) before making them.
- **Storyboards** (M7): `storyboard.render` writes one PNG with a labeled picture of every shot (or every N seconds), so an agent can look at a whole edit at once; over MCP it comes back as an image.

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
- **Resolve:** timelines move between applications as EDL, AAF, XML or OTIO files, imported with File > Import Timeline and exported from File > Export (pp. 527–530, 4159–4160).
  - OTIO comes as `.otio` (the timeline) or `.otioz` (a bundle of the timeline and all of its media).
  - Importing an `.otioz` unpacks its media into the folder the bundle is in and links them.
  - An import creates a new timeline, can apply the file's frame rate to the project, and asks for a folder (searched with its subfolders) when media isn't where the file says.
  - None of this is on the Cut page.
- **vidcut:** OpenTimelineIO only, both ways (`otio.rs`, `bundle.rs`), in the shapes Resolve writes and reads:
  - **Formats:** `.otio` (the timeline), `.otioz` (a zip file of the timeline and its media) and `.otiod` (the same as a folder). They are laid out as OpenTimelineIO's own adapters lay them out: `content.otio`, `version.txt` (zip only), and the media stored uncompressed under `media/`, named by relative paths.
  - **Exporting:** `vidcut export start cut.otio` (or File > Export Timeline) writes:
    - each video track
    - the audio of each video track's clips, as an audio track (J/L cuts become audio clips offset from the video edits)
    - each audio track, and markers
    - in `metadata.vidcut`: the In/Out marks, clip gain/mute and track settings (lock, mute, solo, fader, name)

    A track's video off or audio muted also marks the OTIO track disabled. Unlike media exports it covers the whole timeline unless given a range. An `.otioz` or `.otiod` export (File > Export Timeline with Media) copies the media whole, as Resolve does; it runs as a job.
  - **What Resolve reads besides:**
    - Media are named by plain absolute path, not `file://` URLs, which Resolve doesn't read.
    - Source ranges count from each media file's own start timecode (a camera's `tmcd` track, MXF, a broadcast WAV's time reference), as do the available ranges. Resolve places sources by timecode.
    - In `metadata.Resolve_OTIO`: a link group number joins each clip's video and audio, and the audio names the stream and channels it plays.
    - Clip gain is also written as Resolve's clip volume, track lock and solo as Resolve's.
    - Markers are one frame long, with their note where Resolve keeps it.
    - A still is available for one frame, as Resolve writes it. An image sequence is OTIO's `ImageSequenceReference` (folder, name prefix and suffix, first number, zero padding, rate), whose available range is the picture count at that rate: OTIO counts pictures by truncating the range at the reference's rate, so a project-rate length would be a different number of pictures.
  - **Importing:** `vidcut timeline import cut.otio` (or File > Import Timeline) adds the media to the pool.
    - Video tracks become V1, V2, …, each with the audio that belongs to its clips (the clip's link group, if it has one). Other audio tracks become A1, A2, … (vidcut's own files map back exactly).
    - One project holds one timeline (X2), so the import goes after the last clip, or replaces the timeline with `--replace`; the GUI asks which. An empty timeline takes the file's frame rate, start timecode and track settings.
    - Resolve's channel map picks a clip's audio stream, and its clip volume the gain. Markers on clips and tracks become timeline markers.
  - **Finding media:**
    - Media are named as `file://` URLs (escaped or not), plain paths, Windows paths, paths relative to the file, or the URL Resolve kept for offline media.
    - A project folder moved with its timeline file is found without asking: the path's last parts are looked for next to the file.
    - Other media that moved are found by name with `--search FOLDER` (subfolders too), and the GUI asks for that folder.
    - Only local regular files count as media: a device or a pipe counts as missing, and a clip whose `file://` URL names another host is left out with a warning.
  - **Bundles:** an `.otioz` import unpacks the media into a folder named after the bundle, next to it (`cut.otioz` → `cut/media/…`), or where `--unpack` says. Files already there are kept if they are the bundle's (by CRC-32); other files in the way are an error. A dry run unpacks nothing.
  - **Accepts what Resolve writes:** `Clip.1` and `Clip.2`, source ranges in media timecode, float rates (snapped to exact rationals such as 24000/1001), Resolve's effect lists.
  - **Left out, with a warning each:** transitions (cuts instead), nested stacks (compound clips), generators, speed changes, clip volume keyframes and fades, and clips that would overlap others on their track.
  - Export → import reproduces a timeline frame for frame, through all three formats and through OpenTimelineIO's own Python library (`tests/cli.rs`, `otio/tests.rs`).
- **Why:**
  - OTIO is the user's choice: an open standard that Resolve and other editors read and write, so a cut can move to a finishing tool and back, and agents can write timelines in a documented format instead of vidcut's own.
  - Writing Resolve's own conventions (plain paths, timecode source ranges, link groups) is what lets Resolve link media and keep audio with its video; other OTIO tools read them as plain OTIO.
  - A bundle's media go into a folder of their own rather than next to the bundle as Resolve does: two bundles unpacked in one folder (Downloads, say) would otherwise mix their `media/` folders, and one bundle's media can be moved or deleted as one folder.

## X10. An MCP server for AI agents
- **Resolve:** scripting is a Python/Lua API for Studio (Workspace > Scripts, outside the Cut page chapters).
- **vidcut:** `vidcut mcp` serves the Model Context Protocol on stdin/stdout. Every request method is a tool with its JSON Schema (`edit.split` is `edit_split`), plus `use_project`. Calls run like CLI commands: inside the GUI that has the project open, so the person at it watches each edit, or headless. Frames, screenshots and storyboards come back as images; exports report progress. Cancelling a call stops a headless export, and one routed into the GUI also stops a frame, a storyboard, an analysis and the loudness measure before a foreground export. A background export job keeps running until `jobs.cancel`. AGENTS.md shows how to connect an agent.
- **Why:** the user wants vidcut drivable by AI agents. MCP is how agents discover and call tools; generating the tools from the request schema keeps them complete and in step with the CLI.

## X11. Dynamic Trim and Ripple Delete Silence, simplified
- **Resolve:** Dynamic Trim trims while the program plays, with sound, in a dynamically updating timeline (pp. 730–731). Ripple Delete Silence can crossfade the remaining audio (p. 773).
- **vidcut:**
  - **Dynamic Trim:** J/L run the trim at shuttle speed while the two-up shows the frames on both sides as the trim leaves them, without sound. The timeline draws the trim as it runs, but the project changes once, when K stops it (spec D32).
  - **Ripple Delete Silence:** there is no crossfade option. Its cuts get the 3 ms de-click fades every vidcut cut gets in export.
- **Why:** the preview plays the timeline as one EDL, so changing the cut on every frame of a run would mean reloading it many times a second. The two-up shows exactly what the trim will make (the export engine renders those frames), and one change at the end is one undo step that agents and the command log can follow. Crossfades would need overlapping audio, which the track model doesn't have (spec D14); the de-click fades already keep cuts silent.

## X12. Preferences hold only what vidcut needs
- **Resolve:** the Preferences window has System and User panels with many settings: hardware, media storage, playback, the UI (p. 94). Caches and proxies are set up in the project settings and preferences (pp. 191–218).
- **vidcut:** File > Preferences (Ctrl+,) holds settings for vidcut as a whole, kept in `~/.config/vidcut/preferences.toml`: so far whether the pool's thumbnails are kept on disk, and the size limit of that cache. `preferences.get` and `preferences.set` read and change them, from the CLI (`vidcut preferences`), IPC and MCP like everything else.
- **Why:** a preference belongs here only once vidcut has a behavior to choose. A plain TOML file can be edited by hand, kept in dotfiles, and changed by agents.

## X13. Deinterlacing has one quality
- **Resolve:** each interlaced clip has a Deinterlace Quality (Normal, High, or the Neural Engine) in the Cut page's Inspector, and Normal is always used during playback (p. 741). Clip Attributes can turn deinterlacing on or off per clip (p. 451).
- **vidcut:** frames flagged interlaced are always deinterlaced with one method (`bwdif`, one frame per frame), in the preview and in export alike (spec D34).
- **Why:** one method keeps the preview and export frame-exact with each other, and `bwdif` is the better of FFmpeg's CPU deinterlacers. vidcut has no Inspector to hold a per-clip choice.

## X14. Color is set per export, not managed
- **Resolve:** color management (RCM) gives the timeline a color space. Media come into it through input transforms and tone mapping, and the output color space is chosen for delivery (pp. 230–239).
- **vidcut:** there is no timeline color space and no grading. Each export picks its output, SDR BT.709 or HDR PQ or HLG (`video.hdr`), and every clip is converted to it from its own tags: tone-mapped, or with SDR white at 203 nits (spec D35). The preview is mpv's rendering for the screen, so HDR clips may look a little different there than in an SDR export.
- **Why:** vidcut cuts, and color correction belongs in an editor like Resolve (see docs/roadmap.md). Converting each clip straight to the output keeps exports right for mixed phone, camera and screen footage without color settings.

## X15. The Smart Indicator holds still
- **Resolve:** after a few seconds of idling, the Smart Indicator moves up and down on the ruler, and the edit point strobes green, or red once the clip has reached its start or end frame (p. 696).
- **vidcut:** the indicator is green, or red where a clip at the edit shows its media's first or last frame (`ops::edit_has_handles`). It doesn't move or strobe.
- **Why:** an idle animation keeps the window repainting for as long as it is open, which costs a modest laptop CPU time and battery while nothing happens. The color tells the same.

## X16. The still duration is a project setting
- **Resolve:** the standard still duration is a user preference (Preferences > User > Editing, p. 114).
- **vidcut:** it is in the project's settings (`still_secs`; `vidcut project set --still-duration 3s`).
- **Why:** an edit is then the same in every run, whoever makes it and on whichever computer: an agent appending a still gets the length the project says, not one set in somebody's preferences.

## X17. Image sequences are asked for, not guessed
- **Resolve:** the Media Storage panel's Frame Display Mode decides whether numbered images are one sequence or separate stills: Auto goes by file type (DPX and EXR as sequences, JPEG as stills), or always one way (p. 366).
- **vidcut:** `media.add` adds a still for each image, unless `sequence` asks for the numbered run around it as one clip.
- **Why:** the file type doesn't tell a render's frames from numbered photos or exported slides, and a wrong guess turns a slideshow into a fraction of a second of video, or a render into thousands of stills. A flag makes the CLI and agents predictable.

## X18. Proxies keep source timestamps, and only the preview uses them
- **Resolve:** proxies are generated from the Media Pool. Playback can switch between proxies and originals. Export always reads the originals (pp. 207–218). Resolve's proxy media is a new encode at a chosen resolution; the manual describes generation and the playback switch, not a promise that each proxy frame carries the source frame's presentation timestamp.
- **vidcut:** `proxy.generate` (Media Pool: Generate Proxy) writes an AV1 or H.264 8-bit 4:2:0 file (the preferences' `proxies.codec`: by default AV1 on a hardware encoder, else H.264) for a video or an image sequence, on demand, or by itself in the GUI for the heavy media the preferences' rules pick (decision D41). The default longest side is 960 pixels, even, aspect kept, and a smaller picture is not enlarged. Each proxy frame keeps the source frame's presentation timestamp, so the preview EDL can swap the path and stay frame-exact (decision D41). Preview audio stays on the original file. Stills are left as originals. Files live in `proxies/` beside the project, or in `~/.cache/vidcut/proxies` when that folder is unknown. `project.set use_proxies` (the Media Pool's Proxies checkbox, `vidcut project set --use-proxies true`) is the playback switch. A proxy whose source size or modification time no longer matches is stale and is not played. Export, frames, analysis and OTIO always read the originals.
- **Why:** the preview has to stay frame-exact with export, including image sequences of large pictures that cannot decode in real time. A timestamp-preserving file lets the existing EDL math stand. Leaving audio and export on the originals means a proxy cannot drift from the mix or from the delivered frames.
