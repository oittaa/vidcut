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
- **Paths:** media go offline in Resolve when they move, and are found again with Relink Media (Locate, Disk Search) or Relink Selected Clips (pp. 668–669). vidcut's file stores the media and proxies in or under the project's folder relative to it, so moving or copying the folder, to another computer too, keeps them linked. Media elsewhere stay absolute.
- **Relinking:** Relink Media looks for every missing file by name in a folder and its subfolders in one go, and relinks a file only if it is the one the project knew: the same size, streams (codecs, picture size, rate, audio, duration, timecode) and fingerprint, a SHA-256 of the whole file up to 10 MiB, else of 1 MiB from its start, middle and end (an image sequence: of its first, middle and last pictures). Files named alike in several folders (a camera's `C0001.MP4` on every card) are told apart by that check and by the folders named like the old one's. The others are listed with why they differ, and Relink Anyway uses one; a file picked with Relink… is used either way. The same file keeps its proxy. Media without a fingerprint are checked by size and streams.

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
  - loudness normalization to a target in LUFS (`audio.loudness`; [decisions/agents.md](decisions/agents.md) §5), like the Deliver page's Normalize Audio (p. 4105)

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
- **vidcut:** the start timecode defaults to 00:00:00:00. New Project and File > Project Settings offer 00:00:00:00, 01:00:00:00 and 10:00:00:00, or any other typed; `project.create`, `project.open`'s create and `project.set` take `start_tc` (`--start-tc 01:00:00:00`, drop-frame `01:00:00;00`). It keeps its label when the frame rate or drop-frame counting changes (decision D47), another frame count each time. Timecodes typed into commands are record timecodes, so they follow the setting.
- **Why:** with a zero start, timecode, seconds and frame numbers all count from the same point. Scripts and agents mix all three, and a hidden one-hour offset would make `00:01:00:00` and `60s` mean different frames. The hours offered are where editors and broadcasters start: an hour (programmes, with bars, tone and a countdown before), ten hours (UK broadcast). A start is meant as its label, so a new rate keeps it.

## X8. The source viewer counts project frames
- **Resolve:** Source Clip and Source Tape show a clip at its own frame rate, with its own source timecode (p. 680, 688).
- **vidcut:** source viewers step through *project* frames; for media at the project rate this is the same thing. Source times, marks and the timecode shown count project frames from the clip's first frame.
  - A 23.976 clip in a 30 fps project steps the way it will appear in the timeline: nearest-frame sampling, as export does.
  - Embedded start timecodes are not shown (yet).
- **Why:** vidcut stores every position, including `source_in` and source marks, in project frames ([cut-page-spec.md](cut-page-spec.md), time model). One unit everywhere means what you mark is exactly what lands in the timeline and in the export. The source viewer plays through the same player as the timeline, reading with the export's readers, so the preview matches export frame for frame.

## X9. OpenTimelineIO interchange
- **Resolve:** timelines move between applications as EDL, AAF, XML or OTIO files, imported with File > Import Timeline and exported from File > Export (pp. 527–530, 4159–4160).
  - OTIO comes as `.otio` (the timeline) or `.otioz` (a bundle of the timeline and all of its media).
  - Importing an `.otioz` unpacks its media into a folder named after the bundle, next to it, and links them (Studio 21.1.1; versions before it unpacked into the folder the bundle is in).
  - An import creates a new timeline, can apply the file's frame rate to the project, and asks for a folder (searched with its subfolders) when media isn't where the file says.
  - None of this is on the Cut page.
- **vidcut:** OpenTimelineIO only, both ways (`otio.rs`, `bundle.rs`), in the shapes Resolve writes and reads:
  - **Formats:** `.otio` (the timeline), `.otioz` (a zip file of the timeline and its media) and `.otiod` (the same as a folder). They are laid out as OpenTimelineIO's own adapters lay them out: `content.otio`, `version.txt` (zip only), and the media stored uncompressed under `media/`, named by relative paths.
  - **Exporting:** `vidcut export start cut.otio` (or File > Export Timeline) writes:
    - each video track
    - the audio of each video track's clips, as an audio track (J/L cuts become audio clips offset from the video edits)
    - each audio track, and markers
    - in `metadata.vidcut`: the In/Out marks, clip gain/mute and track settings (lock, mute, solo, fader, name)
    - the clip effects and transitions carried from another editor's file (below)

    A track's video off or audio muted also marks the OTIO track disabled. Unlike media exports it covers the whole timeline unless given a range. An `.otioz` or `.otiod` export (File > Export Timeline with Media) copies the media whole, as Resolve does; it runs as a job.
  - **What Resolve reads besides:**
    - Paths, timecode source ranges (including drop-frame) and marker notes: spec D40.
    - In `metadata.Resolve_OTIO`: a link group number joins each clip's video and audio, and the audio names the stream and channels it plays.
    - Clip gain is also written as Resolve's clip volume, track lock and solo as Resolve's.
    - Tracks carry the names given to them (a video track's audio track its video track's), else the ones Resolve gives its own by place: `Video 1`, `Audio 2`.
    - Resolve's files have no place for the In/Out marks or a track's fader, so Resolve gets neither (checked with Studio 21); vidcut's own import reads them back. Resolve drops other programs' metadata, so a timeline that went through Resolve comes back without them (checked with Studio 21.1.1).
    - A still is available for one frame, as Resolve writes it. An image sequence is OTIO's `ImageSequenceReference` (folder, name prefix and suffix, first number, zero padding, rate), whose available range is the picture count at that rate: OTIO counts pictures by truncating the range at the reference's rate, so a project-rate length would be a different number of pictures.
  - **Importing:** `vidcut timeline import cut.otio` (or File > Import Timeline) adds the media to the pool.
    - Video tracks become V1, V2, …, each with the audio that belongs to its clips (the clip's link group, if it has one). Other audio tracks become A1, A2, … (vidcut's own files map back exactly).
    - One project holds one timeline (X2), so the import goes after the last clip, or replaces the timeline with `--replace`; the GUI asks which. An empty timeline takes the file's frame rate, start timecode and track settings.
    - Resolve's channel map picks a clip's audio stream, and its clip volume the gain. Markers on clips and tracks become timeline markers; markers keep their lengths. Track names other than Resolve's defaults (`Video 1`, `Audio 2`) name the tracks; an audio track whose clips become clips' audio has none to keep it. A marker's note is its `comment`, or `Resolve_OTIO.Note` when that is empty, as Resolve leaves it. A file without a timeline name, as Resolve writes it, names the timeline after the file.
  - **Finding media:**
    - Media are named as `file://` URLs (escaped or not), plain paths, Windows paths, paths relative to the file, or the URL Resolve kept for offline media.
    - A project folder moved with its timeline file is found without asking: the path's last parts are looked for next to the file.
    - Other media that moved are found by name with `--search FOLDER` (subfolders too), and the GUI asks for that folder.
    - Only local regular files count as media: a device or a pipe counts as missing, and a clip whose `file://` URL names another host is left out with a warning.
  - **Bundles:** an `.otioz` import unpacks the media into a folder named after the bundle, next to it (`cut.otioz` → `cut/media/…`), or where `--unpack` says. Files already there are kept if they are the bundle's (by CRC-32); other files in the way are an error. A dry run unpacks nothing.
  - **Clip effects** (Transform, Cropping, Composite, Video Faders, the clip volume's fades, Pan, …; another program's named ones too) stay on the clip's picture or sound: kept as data, not shown or played, and written back unchanged on export. Editing them stays out of scope (spec §1).
    - **Which:** the ones in use: Resolve's when switched on with a setting off its default, Dynamic Zoom whenever it is on. The untouched stack Resolve writes on every clip and switched-off effects are left out.
    - **Shown:** a badge on the clip (`fx Transform, Cropping`), their names in `timeline get` (`effects`), the effects themselves in `project get`, and one import warning naming them.
    - **Through edits:** keyframe times count from the clip's first source frame, so they stay on their source frames through trims, slips and splits. A split leaves the fade-in on the first part and the fade-out on the last, as does an export that cuts a clip. The clip volume's level is the clip gain, written into it. An import at another frame rate converts keyframe times and fade lengths.
    - **In Resolve** (Studio 21.1.1): an import of vidcut's file rebuilds them from the ones in use.
  - **Transitions** (a dissolve as OTIO's `SMPTE_Dissolve`, Resolve's audio crossfades and others as `Custom_Transition`) stay on their edit points, between two clips' pictures or sounds, or between a clip and a gap (a fade from or to black). Shown and played as cuts; written back while they fit.
    - **Fitting:** the clips on each side still meet at the cut (or the gap is still there), the clip after it has the transition's frames before the cut (`in_offset`) as media before its first frame, and the clip before it those after the cut (`out_offset`) after its last. A split of the clip before a cut hands the transition to the part that ends there.
    - **Dropped** when an edit leaves one that no longer fits (a clip moved, deleted, or trimmed to the end of its media), and the edit's `warnings` say how many; an undo brings them back. An import drops those that don't fit the project's media, an export those at its edges.
    - **Shown:** a box over the frames each covers, across the picture or sound band; in `timeline get`, each track's `transitions` with name and cut; one import warning naming them.
    - **In Resolve** (Studio 21.1.1): an import of vidcut's file rebuilds them.
  - **Accepts what Resolve writes:** `Clip.1` and `Clip.2`, source ranges in media timecode, float rates (snapped to exact rationals such as 24000/1001), Resolve's effect lists.
  - **Left out, with a warning each:** nested stacks (compound clips), generators, speed changes, and clips that would overlap others on their track.
  - Export → import reproduces a timeline frame for frame, through all three formats and through OpenTimelineIO's own Python library (`tests/cli.rs`, `otio/tests.rs`).
- **Why:**
  - OTIO is the user's choice: an open standard that Resolve and other editors read and write, so a cut can move to a finishing tool and back, and agents can write timelines in a documented format instead of vidcut's own. Resolve's other formats keep less of a cut: its EDL holds one video track and no audio tracks or markers, its FCPXML (1.10) no markers (checked with Studio 21.1.1).
  - Writing Resolve's own conventions (plain paths, timecode source ranges, link groups) is what lets Resolve link media and keep audio with its video; other OTIO tools read them as plain OTIO. OpenTimelineIO's own bundler wants `file:` URLs and takes a Windows drive letter (`C:\…`) for a URL scheme, so its bundles of vidcut's Windows files need the paths made URLs first (`tests/cli.rs`).
  - Clip effects and transitions are carried rather than left out so that a cut finished in Resolve can come back to vidcut for a recut and go back again without losing them: vidcut needn't understand an effect to keep it. Only the effects in use, because Resolve fills in the rest itself. A transition is dropped rather than moved when an edit takes its cut away, because a dissolve between two other shots is a different edit.
  - A bundle's media go into a folder of their own, as Studio 21.1.1 now does too, rather than next to the bundle as Resolve did before: two bundles unpacked in one folder (Downloads, say) would otherwise mix their `media/` folders, and one bundle's media can be moved or deleted as one folder.

## X10. An MCP server for AI agents
- **Resolve:** scripting is a Python/Lua API for Studio (Workspace > Scripts, outside the Cut page chapters).
- **vidcut:** `vidcut mcp` serves the Model Context Protocol on stdin/stdout. Every request method is a tool with its JSON Schema (`edit.split` is `edit_split`), plus `use_project`. Calls run like CLI commands, inside the GUI that has the project open or headless. Frames, screenshots and storyboards come back as images. Cancelling and progress: [decisions/agents.md](decisions/agents.md) §1. [cli.md](cli.md) shows how to connect an agent.
- **Why:** the user wants vidcut drivable by AI agents. MCP is how agents discover and call tools; generating the tools from the request schema keeps them complete and in step with the CLI.

## X11. Dynamic Trim and Ripple Delete Silence, simplified
- **Resolve:** Dynamic Trim trims while the program plays, with sound, in a dynamically updating timeline (pp. 730–731). Ripple Delete Silence can crossfade the remaining audio (p. 773).
- **vidcut:**
  - **Dynamic Trim:** J/L run the trim at shuttle speed while the two-up shows the frames on both sides as the trim leaves them, without sound. The timeline draws the trim as it runs, but the project changes once, when K stops it (spec D32).
  - **Ripple Delete Silence:** there is no crossfade option. Its cuts get the 3 ms de-click fades every vidcut cut gets in export.
- **Why:** one change at the end keeps the preview, the project file and the undo history out of a run of edits many times a second. The two-up shows exactly what the trim will make (the export engine renders those frames), and one change at the end is one undo step that agents and the command log can follow. Crossfades would need overlapping audio, which the track model doesn't have (spec D14); the de-click fades already keep cuts silent.

## X12. Preferences hold only what vidcut needs
- **Resolve:** the Preferences window has System and User panels with many settings: hardware, media storage, playback, the UI (p. 94). Caches and proxies are set up in the project settings and preferences (pp. 191–218).
- **vidcut:** File > Preferences (Ctrl+,) holds settings for vidcut as a whole, kept in `~/.config/vidcut/preferences.toml`: GPU acceleration (everything on the CPU when off) and GPU decoding (off, the CPU decodes, for a small GPU beside a large CPU), the player's memory for decoded pictures (automatic unless set: [decisions/player.md](decisions/player.md) §3) and its audio device, the thumbnail disk cache and its limit, the timeline's picture memory, and the proxy rules, size and codec. `preferences.get` and `preferences.set` read and change them, from the CLI (`vidcut preferences`), IPC and MCP like everything else.
- **Why:** a preference belongs here only once vidcut has a behavior to choose. A plain TOML file can be edited by hand, kept in dotfiles, and changed by agents.

## X13. Deinterlacing has one quality
- **Resolve:** each interlaced clip has a Deinterlace Quality (Normal, High, or the Neural Engine) in the Cut page's Inspector, and Normal is always used during playback (p. 741). Clip Attributes can turn deinterlacing on or off per clip (p. 451).
- **vidcut:** frames flagged interlaced are always deinterlaced with one method (`bwdif`, one frame per frame), in the preview and in export alike (spec D34).
- **Why:** one method keeps the preview and export frame-exact with each other, and `bwdif` is the better of FFmpeg's CPU deinterlacers. vidcut has no Inspector to hold a per-clip choice.

## X14. Color is set per export, not managed
- **Resolve:** color management (RCM) gives the timeline a color space. Media come into it through input transforms and tone mapping, and the output color space is chosen for delivery (pp. 230–239).
- **vidcut:** there is no timeline color space and no grading. Each export picks its output, SDR BT.709 or HDR PQ or HLG (`video.hdr`), and every clip is converted to it from its own tags: tone-mapped, or with SDR white at 203 nits (spec D35). By default (`auto`) the export decides from the pictures it shows, and its bit depth too (spec D44): Resolve's output is whatever the project's color management says, SDR Rec.709 unless set up otherwise. The viewer converts each picture with the export's own math (HDR tone-mapped as into an SDR export), so HDR clips look there as they will in an SDR export.
- **Why:** vidcut cuts, and color correction belongs in an editor like Resolve (see docs/roadmap.md). Converting each clip straight to the output keeps exports right for mixed phone, camera and screen footage without color settings, and choosing the output from the footage keeps a phone's HDR video HDR without anyone setting up a color space first.

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
- **vidcut:** `proxy.generate` (Media Pool: Generate Proxy) writes an AV1 or H.264 8-bit 4:2:0 file (the preferences' `proxies.codec`: by default AV1 on a hardware encoder, else H.264) for a video or an image sequence, on demand, or by itself in the GUI for the heavy media the preferences' rules pick (decision D41). The default longest side is 960 pixels, even, aspect kept, and a smaller picture is not enlarged. Each proxy frame keeps the source frame's presentation timestamp, so the viewer can swap the path and pick the same frames (decision D41). Preview audio stays on the original file. Stills are left as originals. Files live in `proxies/` beside the project, or in `~/.cache/vidcut/proxies` when that folder is unknown. `project.set use_proxies` (the Media Pool's Proxies checkbox, `vidcut project set --use-proxies true`) is the playback switch. A proxy whose source size or modification time no longer matches is stale and is not played. Export, frames, analysis and OTIO always read the originals.
- **Why:** the preview has to stay frame-exact with export, including image sequences of large pictures that cannot decode in real time. A timestamp-preserving file lets the export's sampling rule pick from it as from the original. Leaving audio and export on the originals means a proxy cannot drift from the mix or from the delivered frames.


## X19. HDR metadata comes from the sources
- **Resolve:** an HDR export carries the metadata of the grading setup: the Mastering Display chosen in Project Settings, HDR10 metadata exported or embedded on the Deliver page (p. 4117), Dolby Vision and HDR10+ from the grade's analysis and trims (pp. 151, 259–260); the Studio version measures MaxCLL and MaxFALL in a Light Level report (p. 282).
- **vidcut:** there is no grade, so an export carries what its sources say (spec D46): the widest mastering display and the highest light levels of the pictures it shows, and their own HDR10+ and Dolby Vision, frame by frame. Where a source says nothing, light levels are measured from its frames, and frames without HDR10+ get statistics measured from them, without the trims a grade would add. Values can be given (`video.mastering_display`, `video.max_cll`).
- **Why:** cutting phone, camera and graded HDR footage keeps it looking on TVs as the sources do, without a colorist's setup; where the sources say nothing, the values come from the pictures rather than a guess.