# Deviations from the Resolve Cut page

vidcut follows the Cut page ([cut-page-spec.md](cut-page-spec.md)) except where an entry here says otherwise: Resolve's behavior with the manual page, vidcut's, and why. Additions that change no Cut-page behavior are listed too; settling what the manual leaves open is a decision (spec §12).

## X1. Projects are always-saved standalone JSON files
- **Resolve:** projects live in a database and are saved explicitly, or by Live Save (Project Manager chapters). Moved media go offline until Relink Media or Relink Selected Clips finds them (pp. 668–669).
- **vidcut:** a project is one `.vidcut` JSON file, written atomically after every change, with no unsaved state. Media under the project's folder are stored relative to it, so the folder can move.
  - Relink Media searches a folder tree by name for every missing file and relinks only the file the project knew: same size, streams and fingerprint (SHA-256 of the file, or of 1 MiB at its start, middle and end when over 10 MiB; a sequence's first, middle and last pictures). Same-named files in several folders (a camera's `C0001.MP4`) are told apart by that and by folder names like the old one's. Others are listed with the reason, for Relink Anyway.
- **Why:** agents and scripts edit projects headless, even while the GUI has them open; with one always-current file nobody disagrees about the project. JSON is diffable and readable.

## X2. One timeline per project file
- **Resolve:** a project holds many timelines, listed in the Master bin (pp. 655, 678).
- **vidcut:** a project file holds exactly one timeline; several timelines are several project files sharing media.
- **Why:** simplicity for a cutting tool, and an obvious mapping for scripts.

## X3. Export dialog beyond Quick Export
- **Resolve:** the Cut page has only Quick Export, which writes one file (Ch. 39, pp. 788–790). Full render settings, Individual clips and the Render Queue are on the Deliver page.
- **vidcut:** Quick Export, in the top bar, works as in Resolve with a shorter row starting with modern codecs: H.265 Master, AV1, ProRes 422 HQ, YouTube 1080p and 2160p, TikTok. H.264 Master, YouTube 720p and Vimeo are in the Export dialog.
  - The Export dialog adds full codec settings, the GPU encoder, ranges, per-clip and audio-only export, a live plan, the export as a shell command, user presets that can join Quick Export (p. 790), and loudness normalization like the Deliver page's Normalize Audio (p. 4105). Every export, CLI ones too, is a job in the Jobs window.
- **Why:** vidcut has no Deliver page; the plan and the command make every GUI export scriptable. A short row is quicker to choose from. AV1 makes the smallest files and GPUs encode it fast; H.265 has replaced H.264 for masters.

## X4. Undo history persists across sessions and CLI runs
- **Resolve:** undo is unlimited within a session and cleared when the project closes (p. 651).
- **vidcut:** the capped snapshot history is kept in a sidecar file under `~/.local/state/vidcut/history/`, surviving restarts and separate CLI runs.
- **Why:** an agent can undo its own edits from a later CLI call, and a human can undo an agent's batch from the GUI.

## X5. Scripting additions (no Cut-page behavior changes)
- **Frame-exact edits:** `edit.insert --at` and `edit.overwrite --at` (Smart Insert inserts only at an edit point, p. 697); `use_marks: false` places by position; `edit.change_duration` takes `end`; `--track` may name the next new track (decision D27); `track.set`/`markers.set` cover the track header, mixer and marker dialog.
- **Live control of the running GUI** ([decisions/gui.md](decisions/gui.md)): CLI commands for an open project run inside it, and `player.*`, `viewer.*`, `selection.*`, `gui.*` and `pool.set` drive it. The command log panel shows every action as its request and copies it as a `vidcut rpc` command.
- **Clips and marks as data:** `edit.copy` returns clips as JSON for `edit.paste --clips` in any project (Ctrl+C also puts it on the system clipboard). Source marks (`media.marks.set`) are saved in the project and used by edits without a range (decision D6).
- **Exports:** `export.plan` (`export --dry-run`), `export --background` with `jobs list|get|wait|cancel` in a running GUI, `presets save` and `presets delete`, and `gui.show` to open Quick Export, Export, Jobs, the log, the Mixer or Preferences.
- **Measuring:** `analyze silence|scenes|black|loudness` and `timeline boring` report findings as frame ranges without changing anything; `storyboard.render` draws every shot in one PNG, so an agent sees a whole edit at once.

## X6. Removing a bin keeps its clips
- **Resolve:** Delete Bin deletes the bin, the bins inside it and their clips (p. 377).
- **vidcut:** Remove Bin (`bins.remove`) moves the media of the bin and the bins inside it up into its parent; the timeline is untouched. `media.remove` removes media, with `force` when timeline clips use them.
- **Why:** bins only organize the pool; removing one by script or by mistake must not take media and their clips out of the project.

## X7. Timelines start at 00:00:00:00
- **Resolve:** new timelines start at record timecode 01:00:00:00 (p. 678).
- **vidcut:** the start defaults to 00:00:00:00; New Project, Project Settings and `start_tc` offer 01:00:00:00, 10:00:00:00 or any other. The start keeps its label when the rate or drop-frame counting changes (decision D47). Typed timecodes are record timecodes.
- **Why:** with a zero start, timecode, seconds and frames count from one point; scripts mix all three, and a hidden hour would make `00:01:00:00` and `60s` different frames.

## X8. The source viewer counts project frames
- **Resolve:** Source Clip and Source Tape show a clip at its own frame rate and source timecode (pp. 680, 688).
- **vidcut:** the source viewers step through project frames from the clip's first frame, sampled as export samples them; embedded start timecodes are not shown.
- **Why:** every position, source marks included, is in project frames, so what you mark is exactly what lands in the timeline and the export.

## X9. OpenTimelineIO interchange
- **Resolve** (pp. 527–530, 4159–4160; not on the Cut page): Import Timeline and Export move timelines as EDL, AAF, XML or OTIO: `.otio`, or `.otioz` with the media, which an import unpacks next to it into a folder named after it. An import makes a new timeline, can take the file's rate, and asks for a folder when media moved.
- **vidcut:** OTIO only, both ways, in the shapes Resolve writes and reads (spec D40; commands: [cli.md](cli.md)): `.otio`, `.otioz` (zip) and `.otiod` (folder), laid out as OpenTimelineIO's adapters do (`content.otio`, `version.txt` in zips, media uncompressed under `media/`).
  - **Export:** each video track with its clips' audio as an audio track (J/L cuts as offset audio clips), each audio track, markers, and in `metadata.vidcut` the In/Out marks, clip gain and mute, and track settings. Tracks with video off or muted are also marked disabled.
  - **For Resolve:** in `metadata.Resolve_OTIO`, a link group joins each clip's video and audio, and the audio names its stream and channels. Clip gain is also its clip volume, track lock and solo its own; unnamed tracks get its defaults (`Video 1`, `Audio 2`). A still is one frame long; an image sequence is an `ImageSequenceReference`.
  - **Resolve drops** `metadata.vidcut`, so the In/Out marks and track faders don't survive it ([todo](todo/otio-track-fader-to-resolve.md)).
  - **Import:** video tracks become V1, V2, … with their clips' audio, other audio tracks A1, A2, …, appended or replacing the timeline (X2); an empty timeline takes the file's rate, start timecode and track settings. It reads Resolve's `Clip.1`/`Clip.2`, channel map, clip volume, marker notes, track names, source timecode and float rates.
  - **Left out, with a warning:** nested stacks, generators, speed changes, overlapping clips (todo `otio-*`), media on another host. Media may be `file://` URLs, plain, Windows or relative paths, or Resolve's offline-media URL; a project folder moved with the file is found by itself, other media with `--search`.
  - **Bundles:** an `.otioz` unpacks next to itself into a folder named after it (`cut/media/…`), or into `--unpack`. Files already there are kept if they are the bundle's (by CRC-32); other files in the way are an error.
  - **Clip effects** (Transform, Cropping, fades, Pan; other programs' too) stay on the clip as data: shown, not applied, written back unchanged, not editable (spec §1). Only those in use are kept. Keyframes stay on their source frames through trims, slips and splits.
  - **Transitions** (`SMPTE_Dissolve`, `Custom_Transition`) stay on their edit points, shown and played as cuts, and are written back while their sides still meet with the media they need. An edit that breaks one drops it, counted in its `warnings`; undo restores it. Resolve rebuilds effects and transitions on import.
- **Why:** OTIO is an open standard Resolve and other editors read and write, so a cut goes to a finishing tool and back, and agents write a documented format; Resolve's EDL keeps one video track, its FCPXML no markers. Effects and transitions are carried so a recut loses nothing. Each bundle gets its own folder so bundles never mix media.

## X10. An MCP server for AI agents
- **Resolve:** scripting is a Python/Lua API for Studio (Workspace > Scripts, outside the Cut page chapters).
- **vidcut:** `vidcut mcp` serves the Model Context Protocol on stdin/stdout, every request method a tool generated from the request schema ([cli.md](cli.md), [decisions/agents.md](decisions/agents.md)).
- **Why:** MCP is how AI agents discover and call tools; generating them from the schema keeps them complete and in step with the CLI.

## X11. Dynamic Trim and Ripple Delete Silence, simplified
- **Resolve:** Dynamic Trim trims while the program plays, with sound, in a dynamically updating timeline (pp. 730–731). Ripple Delete Silence can crossfade the remaining audio (p. 773).
- **vidcut:** Dynamic Trim runs at shuttle speed without sound, the two-up showing both sides, and changes the project once, when K stops it (spec D32). Ripple Delete Silence has no crossfade; its cuts get the 3 ms de-click fades every cut gets.
- **Why:** one change at the end is one undo step agents can follow, not a run of edits many times a second. Crossfades would need overlapping audio, which the track model doesn't have (spec D14).

## X12. Preferences hold only what vidcut needs
- **Resolve:** the Preferences window has System and User panels with many settings (p. 94); caches and proxies are set up in project settings and preferences (pp. 191–218).
- **vidcut:** File > Preferences (Ctrl+,) holds only GPU use, the player's memory and audio device, the thumbnail caches and the proxy rules, in `~/.config/vidcut/preferences.toml` ([cli.md](cli.md)).
- **Why:** a preference belongs here only once vidcut has a behavior to choose. A TOML file can be edited by hand, kept in dotfiles and changed by agents.

## X13. Deinterlacing has one quality
- **Resolve:** each interlaced clip has a Deinterlace Quality (Normal, High, Neural Engine) in the Inspector, and playback always uses Normal (p. 741). Clip Attributes turn deinterlacing on or off per clip (p. 451).
- **vidcut:** frames flagged interlaced are always deinterlaced with `bwdif`, in the preview and export alike (spec D34).
- **Why:** one method keeps the preview and export frame-exact, and `bwdif` is the better of FFmpeg's CPU deinterlacers. There is no Inspector for a per-clip choice.

## X14. Color is set per export, not managed
- **Resolve:** color management gives the timeline a color space; media come in through input transforms, and the output space is chosen for delivery, SDR Rec.709 unless set otherwise (pp. 230–239).
- **vidcut:** no timeline color space and no grading. Each export converts every clip from its own tags to SDR BT.709, HDR PQ or HLG (spec D35), chosen from the footage unless set (spec D44); the viewer uses the same math.
- **Why:** vidcut cuts; color correction belongs in an editor like Resolve. Converting each clip straight to the output keeps mixed phone, camera and screen footage right without color settings.

## X15. The Smart Indicator holds still
- **Resolve:** after a few seconds idle, the Smart Indicator moves up and down on the ruler, and the edit point strobes green, or red once the clip has reached its start or end frame (p. 696).
- **vidcut:** the indicator is green, or red where a clip at the edit shows its media's first or last frame; it doesn't move or strobe.
- **Why:** an idle animation keeps the window repainting, costing CPU time and battery while nothing happens.

## X16. The still duration is a project setting
- **Resolve:** the standard still duration is a user preference (Preferences > User > Editing, p. 114).
- **vidcut:** it is a project setting (`still_secs`; `vidcut project set --still-duration 3s`).
- **Why:** an edit is then the same in every run, whoever makes it and on whichever computer.

## X17. Image sequences are asked for, not guessed
- **Resolve:** Frame Display Mode decides whether numbered images are one sequence or stills: Auto goes by file type (DPX and EXR as sequences, JPEG as stills), or it is always one way (p. 366).
- **vidcut:** `media.add` adds a still for each image unless `sequence` asks for the numbered run as one clip (spec D39).
- **Why:** the file type doesn't tell a render's frames from numbered photos, and a wrong guess makes a slideshow a blink of video or a render thousands of stills.

## X18. Proxies keep source timestamps, and only the preview uses them
- **Resolve:** proxies are generated from the Media Pool, playback can switch between proxies and originals, and export reads the originals (pp. 207–218). The manual doesn't say proxy frames keep the source timestamps.
- **vidcut** (spec D41): proxy frames keep the source's timestamps, so the preview swaps only the path. Only the preview's pictures use proxies, never stale ones; audio, export, frames, analysis and OTIO use the originals.
- **Why:** the preview stays frame-exact with export, even for image sequences too large to decode in real time, and a proxy can't drift from the mix or the delivered frames.

## X19. HDR metadata comes from the sources
- **Resolve:** an HDR export carries the grading setup's metadata: the Mastering Display from Project Settings, HDR10 metadata from the Deliver page (p. 4117), Dolby Vision and HDR10+ from the grade (pp. 151, 259–260). Studio measures MaxCLL and MaxFALL in a Light Level report (p. 282).
- **vidcut** (spec D46): with no grade, an export carries what its sources say, frame by frame, measuring the light levels a source leaves out; `video.mastering_display` and `video.max_cll` set them by hand.
- **Why:** cut HDR footage then looks on TVs as the sources do, without a colorist's setup.
