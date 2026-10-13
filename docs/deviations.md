# Deviations from the Resolve Cut page

vidcut follows the Cut page ([cut-page-spec.md](cut-page-spec.md)) unless an entry here says otherwise. Each entry gives:
- **Resolve:** what it does, with the manual page
- **vidcut:** what vidcut does
- **Why:** why that is better for this tool

Additions that don't change Cut-page behavior, such as extra scripting commands, are listed too. Settling something the manual leaves open is a decision, in §12 of the spec.

## X1. Projects are always-saved standalone JSON files
- **Resolve:** projects live in a project database and are saved explicitly; Live Save is an option (Project Manager chapters). Media that move go offline, and are found again with Relink Media (Locate, Disk Search) or Relink Selected Clips (pp. 668–669).
- **vidcut:**
  - A project is one `.vidcut` JSON file, written atomically after every committed change. There is no "unsaved changes" state.
  - Media and proxies in or under the project's folder are stored relative to it, so moving or copying the folder, to another computer too, keeps them linked. Media elsewhere are stored as absolute paths.
  - Relink Media looks for every missing file by name in a folder and its subfolders, in one go. It relinks a file only if it is the one the project knew: the same size, the same streams (codecs, picture size, rate, audio, duration, timecode) and the same fingerprint. The fingerprint is a SHA-256 of the whole file up to 10 MiB, else of 1 MiB each from its start, middle and end (for an image sequence: of its first, middle and last pictures). Media without a fingerprint are checked by size and streams.
  - Files with the same name in several folders (a camera's `C0001.MP4` on every card) are told apart by that check, and by which folders are named like the old one's.
  - Files that differ are listed with the reason, and Relink Anyway uses one. A file picked with Relink… is used either way. Relinking to the same file keeps its proxy.
- **Why:** agents and scripts edit projects headless through the CLI, sometimes while the GUI has the same project open. With one always-current file, the GUI, the CLI and the user never disagree about what the project contains. JSON is also diffable and readable for agents.

## X2. One timeline per project file
- **Resolve:** a project holds many timelines, listed in the Master bin (pp. 655, 678).
- **vidcut:** a project file holds exactly one timeline. Several timelines are several project files that share media.
- **Why:** simplicity for a cutting tool, and an obvious mapping for scripts.

## X3. Export dialog beyond Quick Export
- **Resolve:** the Cut page has only Quick Export, which writes one file (Ch. 39, pp. 788–790). Full render settings, "Individual clips" and the Render Queue are on the Deliver page.
- **vidcut:**
  - **Quick Export** works as in Resolve: a row of presets, Export, a file dialog, then a progress window (Cancel, or Hide to keep it running).
  - Quick Export and Export are buttons in the top bar, which always shows them, and not also File menu items.
  - The preset tiles show the preset's name and format, not brand logos.
  - The row is shorter than Resolve's and starts with the modern codecs: H.265 Master, AV1 (10-bit, at the timeline's size), ProRes 422 HQ, YouTube 1080p and 2160p, TikTok. H.264 Master, YouTube 720p and Vimeo are presets in the Export dialog, which starts from H.265 Master.
  - **The Export dialog** adds:
    - full codec and container settings, starting from any preset
    - GPU encoder choice
    - range (whole timeline, In–Out, or typed), per-clip export with file-name templates, audio-only export
    - a live plan (`export.plan`: the encoder that will run, the files and their lengths) before anything is written
    - the same export as a shell command, ready to copy
    - saving the settings as a user preset, optionally added to Quick Export (p. 790)
    - loudness normalization to a target in LUFS (`audio.loudness`; [decisions/agents.md](decisions/agents.md) §5), like the Deliver page's Normalize Audio (p. 4105)
  - **Jobs:** every export is a job. The Jobs window lists them with progress, ETA and Cancel, including exports started from the CLI. Quitting while exports run asks first.
- **Why:** vidcut has no Deliver page, and the user needs these export options. The plan and the command make every GUI export reproducible from a script. A short row is quicker to choose from. AV1 makes the smallest files at a given quality, YouTube and other sites take it, and current GPUs encode it in hardware as fast as H.264. H.265 has replaced H.264 for masters.

## X4. Undo history persists across sessions and CLI runs
- **Resolve:** undo is unlimited within a session and cleared when the project closes (p. 651).
- **vidcut:** the snapshot history is kept in a sidecar file under `~/.local/state/vidcut/history/`. It is capped, and survives restarts and separate CLI runs.
- **Why:** an agent can undo its own edits from a later CLI call, and a human can undo an agent's batch from the GUI.

## X5. Scripting additions (no Cut-page behavior changes)
- **Exact-frame insert and overwrite:** `edit.insert --at` and `edit.overwrite --at`, API only. The Cut page's Smart Insert inserts only at an existing edit point (p. 697); scripts need frame-exact placement that doesn't depend on a playhead.
- **Command log panel:** shows every action as the request it sent, from any client (GUI or agent). A click copies it as a `vidcut rpc` command, which teaches the API.
- **Live control of the running GUI** ([decisions/gui.md](decisions/gui.md)):
  - CLI commands for an open project run inside the GUI, so humans and agents edit the same session.
  - Live-only commands drive it: `player.*` (seek, step, play, mute: the viewer's monitoring mute), `viewer.*`, `selection.*`, `gui.screenshot` (the viewer picture, or the whole window), `gui.window`, `gui.log`, and `pool.set` (the bin and sort order the pool shows, which Source Tape follows).
- **Clipboard as data:** `edit.copy` returns the clips as JSON, and `edit.paste --clips` pastes such JSON into any project. In the GUI, Ctrl+C also puts that JSON on the system clipboard.
- **Parameters for scripted edits:** `edit.overwrite`/`edit.ripple_overwrite` take `use_marks: false` to place by position (what a drag and drop does); `edit.change_duration` takes `end`; `track.set`/`markers.set` cover the track header buttons, the mixer and the marker dialog. Edit functions and `edit.move` take `--track`, which may name the next new track (decision D27).
- **Persistent source marks:** a media item's In/Out (`media.marks.set`) are saved in the project and used by edits without a range. Resolve remembers per-clip marks too (decision D6); this makes them scriptable.
- **Export jobs:** in a running GUI, `export --background` returns a job id at once, and `jobs list|get|wait|cancel` follow it. Foreground exports report progress and cancel when the client disconnects. `export.plan` (`export --dry-run`) describes an export without writing anything.
- **User presets from scripts:** `presets save` (from settings, or from another preset with `--set` overrides) and `presets delete`.
- **`gui.show`** opens Quick Export, the Export dialog, Jobs, the command log or the Mixer for the person at the GUI.
- **Measuring as data:** `analyze silence|scenes|black|loudness` report what Ripple Delete Silence, Detect Scene Cuts, a black check and a loudness meter find, as frame ranges, without changing anything. `vidcut timeline boring` lists the Boring Detector's findings. Agents use them to plan edits (markers, cuts, In/Out) before making them.
- **Storyboards:** `storyboard.render` writes one PNG with a labeled picture of every shot (or every N seconds), so an agent can see a whole edit at once; over MCP it comes back as an image.

## X6. Removing a bin keeps its clips
- **Resolve:** Delete Bin deletes the bin, the bins inside it and their clips (p. 377).
- **vidcut:** `bins.remove` (Remove Bin) removes the bin and the bins inside it. Their media move up into the removed bin's parent, and the timeline is untouched. Removing media from the project is `media.remove`, which needs `force` when timeline clips use them.
- **Why:** bins only organize the pool. A bin removed by a script or by mistake must not take media, and the timeline clips cut from them, out of the project. Undo would restore them, but an agent tidying bins shouldn't have to know that.

## X7. Timelines start at 00:00:00:00
- **Resolve:** new timelines start at record timecode 01:00:00:00 (p. 678).
- **vidcut:** the start timecode defaults to 00:00:00:00; `project.set --start-tc 01:00:00:00` changes it. Timecodes typed into commands are record timecodes, so they follow the setting.
- **Why:** with a zero start, timecode, seconds and frame numbers all count from the same point. Scripts and agents mix all three, and a hidden one-hour offset would make `00:01:00:00` and `60s` different frames.

## X8. The source viewer counts project frames
- **Resolve:** Source Clip and Source Tape show a clip at its own frame rate, with its own source timecode (pp. 680, 688).
- **vidcut:** the source viewers step through *project* frames, which for media at the project rate are the same thing. Source times, marks and the timecode shown count project frames from the clip's first frame. Embedded start timecodes are not shown.
  - A 23.976 clip in a 30 fps project steps the way it will appear in the timeline: nearest-frame sampling, as export does.
- **Why:** vidcut stores every position, `source_in` and source marks included, in project frames. With one unit everywhere, what you mark is exactly what lands in the timeline and the export. The source viewer plays through the same player as the timeline, with the export's readers, so the preview matches the export frame for frame.

## X9. OpenTimelineIO interchange
- **Resolve** (pp. 527–530, 4159–4160; not on the Cut page):
  - File > Import Timeline and File > Export move timelines as EDL, AAF, XML or OTIO.
  - OTIO comes as `.otio` (the timeline) or `.otioz` (a bundle of the timeline and all its media).
  - Importing an `.otioz` unpacks its media into a folder named after the bundle, next to it, and links them. An `.otiod` folder is imported by picking its `content.otio`.
  - An import creates a new timeline, can apply the file's frame rate to the project, and asks for a folder (searched with its subfolders) when media aren't where the file says.
- **vidcut:** OpenTimelineIO only, both ways (`otio.rs`, `bundle.rs`), in the shapes Resolve writes and reads. Commands: [cli.md](cli.md).
  - **Formats:** `.otio` (the timeline), `.otioz` (a zip of the timeline and its media) and `.otiod` (the same as a folder). They are laid out as OpenTimelineIO's own adapters lay them out: `content.otio`, `version.txt` (zip only), and the media stored uncompressed under `media/`, named by relative paths.
  - **Export** (`vidcut export start cut.otio`, File > Export Timeline) writes:
    - each video track, and its clips' audio as an audio track (J/L cuts become audio clips offset from the video edits)
    - each audio track, and the markers
    - in `metadata.vidcut`: the In/Out marks, clip gain and mute, and track settings (lock, mute, solo, fader, name)

    A track with its video off or its audio muted is also marked disabled. Unlike a media export, it covers the whole timeline unless given a range. An `.otioz` or `.otiod` export (File > Export Timeline with Media) copies the media whole, as Resolve does, and runs as a job.
  - **For Resolve,** besides that:
    - paths, timecode source ranges (drop-frame too) and marker notes: spec D40
    - in `metadata.Resolve_OTIO`: a link group number joins each clip's video and audio, and the audio names the stream and channels it plays
    - clip gain is also written as Resolve's clip volume, and track lock and solo as Resolve's
    - tracks carry the names given to them, else Resolve's own defaults by place (`Video 1`, `Audio 2`); a video track's audio track takes its video track's name
    - a still is available for one frame, as Resolve writes it
    - an image sequence is OTIO's `ImageSequenceReference` (folder, name prefix and suffix, first number, zero padding, rate). Its available range is the picture count at that rate: OTIO counts pictures by truncating the range at the reference's rate, so a project-rate length would give a different count.
  - **What Resolve loses:** its files have no place for the In/Out marks or a track's fader ([todo](todo/otio-track-fader-to-resolve.md)). It drops other programs' metadata, so a timeline that went through Resolve comes back without `metadata.vidcut`.
  - **Import** (`vidcut timeline import cut.otio`, File > Import Timeline) adds the media to the pool.
    - Video tracks become V1, V2, …, each with the audio that belongs to its clips (by link group when there is one). Other audio tracks become A1, A2, …. vidcut's own files map back exactly.
    - A project holds one timeline (X2), so the import goes after the last clip, or replaces the timeline (`--replace`); the GUI asks which. An empty timeline takes the file's frame rate, start timecode and track settings.
    - Resolve's channel map picks a clip's audio stream, and its clip volume sets the gain.
    - Markers on clips and tracks become timeline markers. A marker's note is its `comment`, or `Resolve_OTIO.Note` when `comment` is empty, as Resolve leaves it.
    - Track names other than Resolve's defaults name the tracks. An audio track whose clips become other clips' audio leaves no track to keep its name.
    - A file without a timeline name, as Resolve writes it, names the timeline after the file.
    - Accepted as Resolve writes them: `Clip.1` and `Clip.2`, source ranges in media timecode, float rates (snapped to exact rationals such as 24000/1001), Resolve's effect lists.
    - **Left out, with a warning each:** transitions (cuts instead), nested stacks (compound clips), generators, speed changes, clip volume keyframes and fades, clips that would overlap others on their track, and clip effects in use, by name. A Resolve effect counts as in use when it is on with a setting off its default; Dynamic Zoom whenever it is on. So the untouched effect stack Resolve writes on every clip raises no warning. Carrying these: the `otio-carry-*`, `otio-compound-clips` and `otio-speed-carry` items in [todo/](todo/).
  - **Finding media:**
    - Media can be named as `file://` URLs (escaped or not), plain paths, Windows paths, paths relative to the file, or the URL Resolve keeps for offline media.
    - A project folder moved together with its timeline file is found without asking: the path's last parts are looked for next to the file.
    - Other media that moved are found by name with `--search FOLDER` (subfolders too); the GUI asks for that folder.
    - Only local regular files count as media: a device or a pipe counts as missing, and a clip whose `file://` URL names another host is left out with a warning.
  - **Bundles:** an `.otioz` import unpacks the media into a folder named after the bundle, next to it (`cut.otioz` → `cut/media/…`), or where `--unpack` says. Files already there are kept if they are the bundle's (by CRC-32); other files in the way are an error. A dry run unpacks nothing.
  - **Round trip:** export then import gives back the timeline frame for frame, in all three formats and through OpenTimelineIO's own Python library (`tests/cli.rs`, `otio/tests.rs`). OpenTimelineIO's bundler wants `file:` URLs and reads a Windows drive letter (`C:\…`) as a URL scheme, so bundling vidcut's Windows files with it needs the paths made URLs first (`tests/cli.rs` does).
- **Why:**
  - OTIO is the user's choice: an open standard that Resolve and other editors read and write, so a cut can go to a finishing tool and back, and agents can write timelines in a documented format instead of vidcut's own. Resolve's other formats keep less of a cut: its EDL holds one video track and no audio tracks or markers, and its FCPXML (1.10) no markers.
  - Writing Resolve's conventions (plain paths, timecode source ranges, link groups) lets Resolve link the media and keep audio with its video. Other OTIO tools read them as plain OTIO.
  - Each bundle unpacks into its own folder, so two bundles unpacked in one folder (Downloads, say) don't mix their `media/` folders, and one bundle's media can be moved or deleted as one folder.

## X10. An MCP server for AI agents
- **Resolve:** scripting is a Python/Lua API for Studio (Workspace > Scripts, outside the Cut page chapters).
- **vidcut:** `vidcut mcp` serves the Model Context Protocol on stdin/stdout. Every request method is a tool with its JSON Schema (`edit.split` is `edit_split`), plus `use_project`. Calls run like CLI commands, inside the GUI that has the project open, or headless. Frames, screenshots and storyboards come back as images. Cancelling and progress: [decisions/agents.md](decisions/agents.md) §1. Connecting an agent: [cli.md](cli.md).
- **Why:** the user wants vidcut drivable by AI agents. MCP is how agents discover and call tools, and generating the tools from the request schema keeps them complete and in step with the CLI.

## X11. Dynamic Trim and Ripple Delete Silence, simplified
- **Resolve:** Dynamic Trim trims while the program plays, with sound, in a dynamically updating timeline (pp. 730–731). Ripple Delete Silence can crossfade the remaining audio (p. 773).
- **vidcut:**
  - **Dynamic Trim:** J/L run the trim at shuttle speed, without sound, while the two-up shows the frames on both sides as the trim leaves them. The timeline draws the trim as it runs, but the project changes once, when K stops it (spec D32).
  - **Ripple Delete Silence:** there is no crossfade option. Its cuts get the 3 ms de-click fades every vidcut cut gets in export.
- **Why:** one change at the end keeps the preview, the project file and the undo history out of a run of edits many times a second. It is one undo step that agents and the command log can follow, and the two-up shows exactly what the trim will make (the export engine renders those frames). Crossfades would need overlapping audio, which the track model doesn't have (spec D14); the de-click fades already keep cuts silent.

## X12. Preferences hold only what vidcut needs
- **Resolve:** the Preferences window has System and User panels with many settings: hardware, media storage, playback, the UI (p. 94). Caches and proxies are set up in the project settings and preferences (pp. 191–218).
- **vidcut:** File > Preferences (Ctrl+,) holds settings for vidcut as a whole, in `~/.config/vidcut/preferences.toml`:
  - GPU acceleration (off: everything on the CPU), and GPU decoding (off: the CPU decodes, for a small GPU beside a large CPU)
  - the player's memory for decoded pictures (automatic unless set: [decisions/player.md](decisions/player.md) §3) and its audio device
  - the thumbnail disk cache and its limit, and the timeline's picture memory
  - the proxy rules, size and codec

  `preferences.get` and `preferences.set` read and change them, from the CLI (`vidcut preferences`), IPC and MCP.
- **Why:** a preference belongs here only once vidcut has a behavior to choose. A plain TOML file can be edited by hand, kept in dotfiles, and changed by agents.

## X13. Deinterlacing has one quality
- **Resolve:** each interlaced clip has a Deinterlace Quality (Normal, High, or the Neural Engine) in the Cut page's Inspector, and playback always uses Normal (p. 741). Clip Attributes can turn deinterlacing on or off per clip (p. 451).
- **vidcut:** frames flagged interlaced are always deinterlaced with one method (`bwdif`, one frame per frame), in the preview and in export alike (spec D34).
- **Why:** one method keeps the preview and export frame-exact with each other, and `bwdif` is the better of FFmpeg's CPU deinterlacers. vidcut has no Inspector to hold a per-clip choice.

## X14. Color is set per export, not managed
- **Resolve:** color management (RCM) gives the timeline a color space. Media come into it through input transforms and tone mapping, and the output color space is chosen for delivery (pp. 230–239). The output is whatever color management says: SDR Rec.709 unless set up otherwise.
- **vidcut:**
  - There is no timeline color space and no grading.
  - Each export picks its output (`video.hdr`): SDR BT.709, or HDR PQ or HLG. Every clip is converted to it from its own tags: tone-mapped, or with SDR white at 203 nits (spec D35).
  - By default (`auto`) the export chooses the output, and the bit depth, from the pictures it shows (spec D44).
  - The viewer converts each picture with the export's math (HDR tone-mapped as for an SDR export), so HDR clips look as they will in an SDR export.
- **Why:** vidcut cuts; color correction belongs in an editor like Resolve (see [roadmap.md](roadmap.md)). Converting each clip straight to the output keeps exports right for mixed phone, camera and screen footage without color settings. Choosing the output from the footage keeps a phone's HDR video HDR without anyone setting up a color space first.

## X15. The Smart Indicator holds still
- **Resolve:** after a few seconds of idling, the Smart Indicator moves up and down on the ruler, and the edit point strobes green, or red once the clip has reached its start or end frame (p. 696).
- **vidcut:** the indicator is green, or red where a clip at the edit shows its media's first or last frame (`ops::edit_has_handles`). It doesn't move or strobe.
- **Why:** an idle animation keeps the window repainting as long as it is open, which costs a modest laptop CPU time and battery while nothing happens. The color tells the same.

## X16. The still duration is a project setting
- **Resolve:** the standard still duration is a user preference (Preferences > User > Editing, p. 114).
- **vidcut:** it is a project setting (`still_secs`; `vidcut project set --still-duration 3s`).
- **Why:** an edit is then the same in every run, whoever makes it and on whichever computer: an agent appending a still gets the length the project says, not one from somebody's preferences.

## X17. Image sequences are asked for, not guessed
- **Resolve:** the Media Storage panel's Frame Display Mode decides whether numbered images are one sequence or separate stills: Auto goes by file type (DPX and EXR as sequences, JPEG as stills), or it is always one way (p. 366).
- **vidcut:** `media.add` adds a still for each image, unless `sequence` asks for the numbered run around it as one clip (spec D39).
- **Why:** the file type doesn't tell a render's frames from numbered photos or exported slides. A wrong guess turns a slideshow into a fraction of a second of video, or a render into thousands of stills. A flag makes the CLI and agents predictable.

## X18. Proxies keep source timestamps, and only the preview uses them
- **Resolve:** proxies are generated from the Media Pool, playback can switch between proxies and originals, and export always reads the originals (pp. 207–218). A proxy is a new encode at a chosen resolution; the manual doesn't say its frames keep the source frames' timestamps.
- **vidcut** (spec D41):
  - Each proxy frame keeps the source frame's presentation timestamp, so the preview swaps only the path and picks the same frames.
  - Only the preview plays proxies, and only their pictures. Preview audio, export, frames, analysis and OTIO use the originals.
  - A proxy whose source size or modification time no longer matches is stale and is not played.
  - The running GUI makes proxies by itself for heavy media, by the preferences' rules.
- **Why:** the preview has to stay frame-exact with export, including image sequences of large pictures that can't decode in real time. A file that keeps the timestamps lets the export's sampling rule pick from it as from the original. With audio and export on the originals, a proxy can't drift from the mix or the delivered frames.

## X19. HDR metadata comes from the sources
- **Resolve:** an HDR export carries the grading setup's metadata: the Mastering Display chosen in Project Settings, HDR10 metadata exported or embedded on the Deliver page (p. 4117), and Dolby Vision and HDR10+ from the grade's analysis and trims (pp. 151, 259–260). Studio measures MaxCLL and MaxFALL in a Light Level report (p. 282).
- **vidcut** (spec D46): there is no grade, so an export carries what its sources say: the widest mastering display and the highest light levels of the pictures it shows, and their own HDR10+ and Dolby Vision, frame by frame. Where a source says nothing, its light levels and HDR10+ are measured from its frames, without the trims a grade would add. `video.mastering_display` and `video.max_cll` set values by hand.
- **Why:** cut phone, camera and graded HDR footage then looks on TVs as the sources do, without a colorist's setup. Where the sources say nothing, the values come from the pictures rather than a guess.
