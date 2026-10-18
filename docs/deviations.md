# Deviations from the Resolve Cut page

vidcut follows the Cut page ([cut-page-spec.md](cut-page-spec.md)) unless an entry here says otherwise: what Resolve does, with the manual page, what vidcut does, and why. Additions are listed too. What the manual leaves open is a decision ([spec §12](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)), not a deviation.

## X1. Projects are always-saved standalone JSON files
- **Resolve:** projects live in a project database, saved explicitly or by Live Save (Project Manager chapters). Media that move go offline until relinked (pp. 668–669).
- **vidcut:** a project is one `.vidcut` JSON file, written atomically after every committed change. Media in or under the project's folder are stored relative to it, so the folder can move, to another computer too. Relink Media finds missing files by name and relinks only the same file ([design/model.md](design/model.md#fingerprints-and-relinking)).
- **Why:** scripts and agents edit projects through the CLI while the GUI may have them open; one always-current file keeps them all agreeing. JSON is diffable and readable.

## X2. One timeline per project file
- **Resolve:** a project holds many timelines, listed in the Master bin (pp. 655, 678).
- **vidcut:** a project file holds exactly one timeline.
- **Why:** simplicity for a cutting tool, and an obvious mapping for scripts; several timelines are several project files sharing media.

## X3. Export dialog beyond Quick Export
- **Resolve:** the Cut page has only Quick Export (Ch. 39, pp. 788–790). Render settings, individual clips, user presets, Normalize Audio and the Render Queue are on the Deliver page (pp. 790, 4105, 4130).
- **vidcut:** Quick Export works as on the Cut page, with a shorter row: H.265 Master, AV1, ProRes 422 HQ, YouTube 1080p and 2160p, TikTok. An Export dialog adds every codec, container and encoder setting from any preset, a range, per-clip and audio-only export, loudness normalization, the live plan, the export as a shell command, and user presets. In a running GUI, exports are jobs, one encoding at a time ([design/export.md](design/export.md#the-guis-exports), [design/live.md](design/live.md#jobs)). No progress dialog opens: the Jobs button in the top bar counts running jobs and glows a moment when one starts or ends, and Jobs shows each one's progress and, expanded, its encoder, decoders and GPU use.
- **Why:** vidcut has no Deliver page, and the user wants these options. A dialog over the timeline for every export interrupts the editing it runs beside. The plan and the command make GUI exports reproducible from scripts. A short row is quicker to choose from: AV1 makes the smallest files and current GPUs encode it; H.265 has replaced H.264 for masters.

## X4. Undo history persists across sessions and CLI runs
- **Resolve:** undo is unlimited within a session and cleared when the project closes (p. 651).
- **vidcut:** the history is a file in the state folder, so it survives restarts and separate CLI runs: up to 131,072 steps, then the oldest 10% go ([design/requests.md](design/requests.md#undo-history)).
- **Why:** an agent can undo its own edits from a later CLI call, and a human can undo an agent's batch from the GUI.

## X5. Scripting additions (no Cut-page behavior changes)
- **Resolve:** none on the Cut page.
- **vidcut:**
  - `edit insert --at` and `edit overwrite --at` place at an exact frame; Smart Insert inserts only at an edit point (p. 697).
  - The command log shows every action, the GUI's or an agent's, as its request, copyable as a `vidcut rpc` command.
  - Commands for a project open in a GUI run inside it, and live-only ones drive the GUI ([cli.md](cli.md), [design/live.md](design/live.md)).
  - `edit copy` returns clips as JSON for `edit paste --clips` into any project; the GUI's Ctrl+C also puts it on the system clipboard.
  - Overwrites with `--use-marks false` place by position, as a drag does; `--track` may name the next new track ([D27](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)); `track set` and `markers set` cover the track header, the mixer and the marker dialog; source marks are saved ([D6](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - In a running GUI, `export start --background` returns a job id for `jobs list|get|wait|cancel`; `export plan` writes nothing. `presets save|delete`, `gui show|close`.
  - `analyze silence|scenes|black|loudness` and `timeline boring` report silences, shots, black, loudness and the Boring Detector's clips without editing; `storyboard render` draws every shot into one PNG.
- **Why:** scripts and agents need frame-exact edits without a playhead, and measurements to plan with. The command log teaches the API.

## X6. Removing a bin keeps its clips
- **Resolve:** Delete Bin deletes the bin, the bins inside it and their clips (p. 377).
- **vidcut:** `bins.remove` removes the bin and those inside it; their media move up to its parent. Removing media is `media.remove`, with `force` when clips use them.
- **Why:** bins only organize the pool: removing one, by script or by mistake, must not take media and their clips out of the project.

## X7. Timelines start at 00:00:00:00
- **Resolve:** new timelines start at record timecode 01:00:00:00 (p. 678).
- **vidcut:** the start timecode defaults to 00:00:00:00. New Project and Project Settings offer 00, 01 and 10 hours or any typed timecode (`start_tc`). Timecodes typed into commands are record timecodes.
- **Why:** with a zero start, timecode, seconds and frame numbers count from one point, so `00:01:00:00` and `60s` are the same frame. One hour (programmes) and ten (UK broadcast) are the usual other starts.

## X8. The source viewer counts project frames
- **Resolve:** Source Clip and Source Tape show a clip at its own frame rate, with its own source timecode (pp. 680, 688).
- **vidcut:** source viewers step through project frames, counted from the clip's first frame and sampled as export samples them ([design/export.md](design/export.md#which-source-frame)), which for media at the project rate are its own. Embedded start timecodes aren't shown.
- **Why:** every position, source ones too, is in project frames ([design/model.md](design/model.md#time)), so what you mark is exactly what lands in the timeline and the export.

## X9. OpenTimelineIO interchange
- **Resolve:** timelines move between applications as EDL, AAF, XML or OTIO files through File > Import Timeline and File > Export (pp. 527–530, 4159–4160), off the Cut page. An import makes a new timeline; an `.otioz` bundle unpacks into a folder named after it.
- **vidcut:** OpenTimelineIO only, both ways, in Resolve's shapes ([D40](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)): `.otio`, `.otioz` bundles and `.otiod` bundle folders ([design/model.md](design/model.md#opentimelineio)).
  - File > Export Timeline (`export start cut.otio`), with Media a bundle. Marks, faders and the rest of the project go in vidcut's metadata, which Resolve doesn't keep.
  - File > Import Timeline (`timeline import`) appends, replaces the timeline, adds one compound clip or brings only the media: a project holds one timeline (X2). An OTIO file also opens as a new project.
  - Clip effects, transitions, generators, compound clips and speed changes come in as data and are written back while they fit; Resolve Studio 21.1.1 rebuilds them. Dissolves, cross fades ([D50](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)) and speeds ([D48](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)) play; other transitions play as cuts, generators as gaps, other effects not at all.
- **Why:**
  - OTIO is an open standard Resolve and other editors read and write, so a cut can go to a finishing tool and back, and agents write timelines in a documented format. Resolve's EDL keeps one video track and no audio tracks or markers, its FCPXML (1.10) no markers.
  - Carrying what vidcut doesn't model lets a cut finished in Resolve come back for a recut and return intact. Only effects in use are kept: Resolve fills in the rest. A transition whose cut an edit removes is dropped, not moved: a dissolve between two other shots is another edit.
  - A bundle's media get their own folder, so bundles unpacked side by side don't mix and each moves or deletes whole.

## X10. An MCP server for AI agents
- **Resolve:** none on the Cut page; Studio has a Python/Lua scripting API (Workspace > Scripts).
- **vidcut:** `vidcut mcp` serves the Model Context Protocol on stdin/stdout, every request method a tool ([design/requests.md](design/requests.md), [cli.md](cli.md)).
- **Why:** MCP is how agents discover and call tools; tools generated from the request schema stay complete and in step with the CLI.

## X11. Dynamic Trim and Ripple Delete Silence, simplified
- **Resolve:** a Dynamic Trim run changes the project as it goes, each run one undo step (pp. 730–731). Ripple Delete Silence can crossfade the remaining audio (p. 773).
- **vidcut:** a Dynamic Trim run plays, and the timeline and the viewer show it, as it goes; the project changes once, when the run stops ([D32](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)). Ripple Delete Silence has no crossfade; its cuts get the 3 ms de-click fades every cut gets.
- **Why:** one change at the end keeps the file, the history and other clients out of edits many times a second, and is the same one undo step. The de-click fades already keep the cuts silent.

## X12. Preferences hold only what vidcut needs
- **Resolve:** Preferences has System and User panels with many settings (p. 94). Caches and proxies are set up in project settings and preferences (pp. 191–218).
- **vidcut:** File > Preferences (Ctrl+,) holds the theme, GPU use, the player's memory and audio device, the thumbnail cache, and the proxy rules and folder, in `preferences.toml` ([design/model.md](design/model.md#preferences)); `preferences get|set` reach them from scripts. Reset vidcut… there (and `vidcut reset`) goes back to a fresh install's state: the preferences, caches, undo histories (but those of open projects), recent projects, view settings and remembered folders; presets and themes only when asked, and never a proxy folder you set. A confirmation comes first, and Esc cancels it. It refuses while another vidcut GUI runs, which would write its settings back.
- **Why:** a preference belongs here only once vidcut has a behavior to choose. A plain TOML file can be edited by hand, kept in dotfiles and changed by agents. A reset makes a clean start, for testing too, one step instead of finding each file.

## X13. Deinterlacing has one quality
- **Resolve:** each interlaced clip has a Deinterlace Quality (Normal, High, Neural Engine) in the Inspector; playback always uses Normal (p. 741), and Clip Attributes switch deinterlacing per clip (p. 451).
- **vidcut:** frames flagged interlaced are always deinterlaced with `bwdif`, in the preview and export alike ([D34](cut-page-spec.md#12-decisions-on-gaps-and-contradictions); on the GPU: [design/gpu.md](design/gpu.md#deinterlacing-and-hdr)).
- **Why:** one method keeps preview and export frame-exact, and `bwdif` is the better of FFmpeg's CPU deinterlacers. There is no Inspector for a per-clip choice.

## X14. Color is set per export, not managed
- **Resolve:** color management gives the timeline a color space; media come in through input transforms and tone mapping, and the output space is chosen for delivery (pp. 230–239).
- **vidcut:** no timeline color space and no grading. Each export picks SDR, PQ or HLG from the footage it shows unless told ([D44](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)), and converts each clip to it from its own tags ([D35](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)); the viewer uses the same math.
- **Why:** vidcut cuts; grading belongs elsewhere. Mixed phone, camera and screen footage comes out right without color settings, and a phone's HDR video stays HDR.

## X15. The Smart Indicator holds still
- **Resolve:** after a few idle seconds, the Smart Indicator moves up and down on the ruler and the edit point strobes green, or red once a clip reaches its start or end frame (p. 696).
- **vidcut:** the indicator is green, or red where a clip at the edit shows its media's first or last frame. It doesn't move or strobe.
- **Why:** an idle animation repaints the window all the time, which costs a laptop CPU time and battery. The color tells the same.

## X16. The still duration is a project setting
- **Resolve:** the standard still duration is a user preference (Preferences > User > Editing, p. 114).
- **vidcut:** it is a project setting (`project set --still-duration 3s`; [D38](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:** an edit is the same in every run on every computer: an agent appending a still gets the project's length.

## X17. Image sequences are asked for, not guessed
- **Resolve:** the Frame Display Mode decides whether numbered images are one sequence or stills; Auto goes by file type (p. 366).
- **vidcut:** each image is a still unless `media add --sequence` asks for the numbered run around it, or `media add-folder --sequence` for the runs in a folder ([D39](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)). Importing numbered images in the GUI, picked or in a folder, asks which, with one clip each already chosen for frame formats (PNG, TIFF, EXR, DPX, TGA, BMP).
- **Why:** the file type doesn't tell a render's frames from numbered photos, and a wrong guess turns a slideshow into a blink or a render into thousands of stills. A flag keeps scripts predictable.

## X18. Proxies keep source timestamps, and only the preview uses them
- **Resolve:** proxies are generated from the Media Pool, playback switches between them and the originals, and export reads the originals (pp. 207–218). The manual doesn't say whether proxy frames keep their sources' timestamps.
- **vidcut:** they keep them, so the preview picks the same frames. Only the preview's pictures use proxies; its sound, export, frames, analysis and OTIO read the originals ([D41](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:** the preview stays frame-exact with export, even for image sequences too large to decode in real time, and can't drift from the mix or the delivered frames.

## X19. HDR metadata comes from the sources
- **Resolve:** an HDR export carries the grading setup's metadata: the Mastering Display, HDR10 metadata (p. 4117), Dolby Vision and HDR10+ from the grade (pp. 151, 259–260). Studio measures MaxCLL and MaxFALL (p. 282).
- **vidcut:** there is no grade, so an export carries what its sources say, or measures it from their frames ([D46](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:** phone, camera and graded HDR footage keeps looking on TVs as its sources do, without a colorist's setup.

## X20. Stats for nerds
- **Resolve:** none: the viewer shows nothing of how it plays.
- **vidcut:** the viewer's right-click menu opens a panel over the picture, a group each for decoding, the picture, the viewer, playback, the pictures kept and the sound. `player stats` toggles it; `player state` has the same facts as `stats`.
- **Why:** where the GPU decodes and draws differs between machines; this shows it without a debug log.

## X21. Ctrl+wheel zooms the timeline
- **Resolve:** Option+wheel zooms the timeline and Command+wheel moves through it (Edit page, p. 853); the Cut page's lower timeline zooms with Option+wheel too.
- **vidcut:** Ctrl+wheel (Cmd on a Mac) and a trackpad's pinch zoom the lower timeline.
- **Why:** Ctrl+wheel and Cmd+wheel zoom in browsers and most programs, and egui gives them and a pinch as one zoom.

## X22. Themes
- **Resolve:** none: one dark gray look.
- **vidcut:** nine built-in themes and your own TOML files. Native, the default, follows the computer's light or dark mode and accent color; Classic is a flat dark gray like the Cut page's. Preferences or `preferences set appearance.theme=…` switch at once.
- **Why:** a desktop app is expected to offer a few looks for long sessions. Every theme keeps the Cut page's layout and what its colors mean.

## X23. A Keyboard Shortcuts window
- **Resolve:** none on the Cut page; the Keyboard Customization window lists and changes keys, without explanations.
- **vidcut:** F1, or Help in the top bar, lists every shortcut, one action a row. A ? beside a less obvious action explains it in a few plain words. Keys can't be changed.
- **Why:** newcomers learn the keys and the ideas behind them in one place, without leaving the app.

## X24. Cross fades follow their curves exactly
- **Resolve:** Studio 21.1.1 renders a Cross Fade's curve as straight lines between points on it: the first sample, every 1000 from there and from the edit, and the last (at 48 kHz). Early in a +3 dB fade the gain is up to 0.06 off.
- **vidcut:** each sample's gain is the curve's ([D50](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:** the curve is what the transition is named for, and needs no editor's points. The two differ by more than 1% only in the 20 ms where a curve leaves or reaches silence.

## X25. Small additions to the layout
- **Resolve:** the Import Media and Import Media Folder buttons sit in the Media Pool (p. 654); the transport bar has no Clear; Timeline Options (p. 645) have no filmstrip or waveform toggles, and Snap is there, not among the timeline buttons (p. 649); the pool doesn't sort by duration.
- **vidcut:** Import and Import Folder in the top bar; Clear in the transport bar clears the marks (Alt+X); Filmstrips and Waveforms in Timeline Options; a Snap button among the timeline buttons as well as Snap in Timeline Options; Duration among the pool's sorts. File dialogs for media, projects and exports each start in the folder last used for them.
- **Why:** each puts one click on something the Cut page hides in a menu or leaves out, and none changes how an edit works.

## X26. The proxy folder is a preference
- **Resolve:** proxies go to the project's "Proxy generation location" (Project Settings, Working Folders, p. 145), in subfolders by their sources' folders (p. 208). Preferences can put them in a "Proxy" subfolder beside each source instead, or ask for a folder each time (p. 98).
- **vidcut:** one preference, `proxies.folder`: `proxies` in the project's folder by default; a relative folder is in the project's folder, an absolute one is shared by every project. A proxy is named by its source's content fingerprint, size and modification time, and the project names it by file alone, so projects that use the same file find and play one proxy, and each computer finds it in its own folder.
- **Why:** where proxies go depends on the computer's disks, not the project. A project on a network drive opens on several computers, and a folder saved in it would name one computer's disk. One setting serves both reasons to move proxies: keeping them on a local disk while projects and media sit on a network drive, and making one proxy for intros and outros that several projects use.

## X27. A dissolve short of media is shortened
- **Resolve:** Studio 21.1.1's Cut page adds no dissolve, and says nothing, when the clips lack media for the whole second at the edit; with none at all past the cut, Ctrl+T on one selected side adds the whole second anyway, the missing frames black. The manual says a transition is as long as the clips' media allow (p. 1159), and on the Edit page Ctrl+T asks to trim the clips or skip the edit (p. 1160).
- **vidcut:** the dissolve is shortened to the media each side has, as the manual says ([D50](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)). With no media past the cut on either side, the Dissolve button and Ctrl+T say so and add nothing.
- **Why:** a key that does nothing without a word leaves the editor guessing, and a dissolve through black is a different transition from the one asked for.

## X28. Up/Down in Dynamic Trim move the trim
- **Resolve:** on the Cut page (21.1.1), Up/Down in Dynamic Trim move only the playhead; the selected edit stays, and the next J or L takes the playhead back to it. Down during a run rolls the edit to the next edit with the playhead, removing the clip between. The manual says Up/Down move both the selection and the playhead (p. 731).
- **vidcut:** as the manual says: Up/Down stop a run and apply it, then select the next edit on the same track and move the playhead to it.
- **Why:** the keyboard then picks the next edit to trim, as the manual describes, and no clip disappears.

## X29. A Dynamic Trim run says why it stopped
- **Resolve:** a run that reaches a neighbor's last frame or the end of the media just stops; the Trim Editor comes back, its handles green, and nothing says why (21.1.1).
- **vidcut:** the run stops there and applies; the cut turns red and the viewer says why, in the Trim Editor's line or under the picture, until the next trim ([D32](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:** as with [X27](#x27-a-dissolve-short-of-media-is-shortened), a stop without a word leaves the editor guessing whether a key was missed or the media ran out.
