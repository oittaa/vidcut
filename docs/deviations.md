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
  - Clip effects, transitions, generators, compound clips and speed changes come in as data and are written back while they fit. Dissolves, cross fades ([D50](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)) and speeds ([D48](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)) play; other transitions play as cuts, generators as gaps, other effects not at all.
  - Resolve Studio 21.1.1 imports `.otio` and `.otioz` files, and an `.otiod` folder by its `content.otio`. It keeps tracks and their names, J/L cuts, clip gain, markers, stills, image sequences, sources' timecode, clip effects and their keyframes, transitions, generators, speeds, and compound clips with their picture and sound linked. Its own OTIO leaves out ResolveFX plugins, so those never come over. OTIO has no drop-frame flag: an imported timeline takes the project's frame rate and drop-frame setting, and an import into a project with no media turns drop-frame off.
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
- **vidcut:** a Dynamic Trim run plays, and the timeline and the viewer show it, as it goes; the project changes once, when the run stops ([D32](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)). Ripple Delete Silence has no crossfade; its cuts get the 3 ms de-click fades every cut gets. It can also cut what isn't speech ([X47](#x47-ripple-delete-silence-can-keep-only-speech)), and its stripes can be edited before Remove ([X48](#x48-ripple-delete-silences-stripes-can-be-edited-before-remove)).
- **Why:** one change at the end keeps the file, the history and other clients out of edits many times a second, and is the same one undo step. The de-click fades already keep the cuts silent.

## X12. Preferences hold only what vidcut needs
- **Resolve:** Preferences has System and User panels with many settings (p. 94). Caches and proxies are set up in project settings and preferences (pp. 191–218).
- **vidcut:** File > Preferences (Ctrl+,) holds the theme, GPU use, the player's memory and audio device, the thumbnail cache, and the proxy rules and folder, in `preferences.toml` ([design/model.md](design/model.md#preferences)); `preferences get|set|reset` reach them from scripts. When the file can't be read, the window shows why, with Show in Folder and Reset Preferences (the defaults; the old file is kept as `preferences.toml.bak`, or `.bak.2` and on, never replacing a backup). Reset vidcut… there (and `vidcut reset`) goes back to a fresh install's state: the preferences, caches, undo histories (but those of open projects), recent projects, view settings, the window's size and place, and remembered folders; presets and themes only when asked, and never a proxy folder you set. A confirmation comes first, and Esc cancels it. It refuses while another vidcut GUI runs, which would write its settings back.
- **Why:** a preference belongs here only once vidcut has a behavior to choose. A plain TOML file can be edited by hand, kept in dotfiles and changed by agents. A reset makes a clean start, for testing too, one step instead of finding each file.

## X13. Deinterlacing has one quality
- **Resolve:** each interlaced clip has a Deinterlace Quality (Normal, High, Neural Engine) in the Inspector; playback always uses Normal (p. 741), and Clip Attributes switch deinterlacing per clip (p. 451).
- **vidcut:** frames flagged interlaced are always deinterlaced with `bwdif`, in the preview and export alike ([D34](cut-page-spec.md#12-decisions-on-gaps-and-contradictions); on the GPU: [design/gpu.md](design/gpu.md#deinterlacing-and-hdr)).
- **Why:** one method keeps preview and export frame-exact, and `bwdif` is the better of FFmpeg's CPU deinterlacers. There is no Inspector for a per-clip choice.

## X14. Color is set per export, not managed
- **Resolve:** color management gives the timeline a color space; media come in through input transforms and tone mapping, and the output space is chosen for delivery (pp. 230–239).
- **vidcut:** no timeline color space and no grading. Each export picks SDR, PQ or HLG from the footage it shows unless told ([D44](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)), and converts each clip to it from its own tags, or its media's Input Color Space where they are wrong ([D35](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)); the viewer uses the same math.
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
- **Resolve:** the Import Media and Import Media Folder buttons sit in the Media Pool (p. 654), and on the Cut page Import Media Folder has no menu item (Studio 21.1.1); the transport bar has no Clear; Timeline Options (p. 645) have no filmstrip or waveform toggles, and Snap is there, not among the timeline buttons (p. 649); the pool doesn't sort by duration.
- **vidcut:** Import and Import Folder in the top bar, and Import Media Folder… on the File menu and the pool's right-click menu; Clear in the transport bar clears the marks (Alt+X); Filmstrips and Waveforms in Timeline Options; a Snap button among the timeline buttons as well as Snap in Timeline Options; Duration among the pool's sorts. File dialogs for media, projects and exports each start in the folder last used for them.
- **Why:** each puts one click on something the Cut page hides in a menu or leaves out, or a menu item where it has only a button, and none changes how an edit works.

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

## X30. Slowed sound keeps its pitch on every system
- **Resolve:** the manual says K held with L plays forward slowly with pitch-corrected sound on macOS (p. 871), so elsewhere the slowed sound is lower; K held with J plays backward with slowed sound. Studio 21.1.1 on Windows plays it slowed; its pitch wasn't checked.
- **vidcut:** sound played at any speed but 1×, slow play included, keeps its pitch on every system, backward too ([D54](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:** one behavior everywhere, and slowed speech at its own pitch stays easy to follow when listening for a cut.

## X31. A file is in the pool once
- **Resolve:** importing a file into another bin adds a second item for it, with its own media id; importing it into the bin that holds it adds nothing (Studio 21.1.1, through its scripting API).
- **vidcut:** a file, or an image sequence, is one pool item. Importing it again keeps its id; Import Media Folder, or an import into a bin, moves it into that bin ([D53](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:** scripts, the command log and OTIO files name media by id. Two items for one file would split its marks, color and proxy, and an agent importing a folder again would double the pool.

## X32. Import Media Folder names what it left out
- **Resolve:** a file it can't read, a text file or one that only looks like a video, is left out without a word, and a folder without media imports nothing (Studio 21.1.1, through its scripting API).
- **vidcut:** the result names each file skipped and why; the GUI's notice counts them, names up to five of each kind, and lists each with why when hovered. A folder without media is an error ([D53](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:** as with [X29](#x29-a-dynamic-trim-run-says-why-it-stopped): a card or render folder imported for an agent to cut must not lose a clip unnoticed.

## X33. Each undo step says what, where and how much
- **Resolve:** Studio 21.1.1 names a step by its kind: "Split Clips", "Remove clips", "Add Transition to Edit Points"; only Append names the media. Edit > Undo and Redo don't say what they would undo or redo. Ctrl+Z shows nothing.
- **vidcut:** a step says what it did, to which clip (its name, else its media's), where (timecode and track) and by how much: "Trim the end of Interview.mp4 by -12 frames at 00:01:02:10 on V1". An `apply` batch lists its commands under it. Edit > Undo and Redo name the step. Ctrl+Z and Ctrl+Shift+Z say what they undid or redid and briefly outline where it was in the timeline ([D55](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:** ten lines of "Split Clips" don't say which one to go back to, and an undo whose change is off screen, or was an agent's, looks like nothing happened.

## X34. The undo history says who made each step, and when
- **Resolve:** one editor makes every step; the History window lists labels alone (21.1.1).
- **vidcut:** each step keeps who made it (the GUI, the CLI, an MCP client by its own name, another program over IPC, or a program that wrote the project file) and when. The History window badges the steps the GUI didn't make, says how long ago each was made, and marks where each GUI session began ("Opened Tue 14:02"), since the history outlives sessions ([X4](#x4-undo-history-persists-across-sessions-and-cli-runs)). Steps one client made in a row, such as an agent's forty edits, fold into one row that opens to show them, and its Undo all goes back to before the first. A script or agent on the CLI names itself with `--client` (`VIDCUT_CLIENT`). `history.get` gives all of it, and `history.go` goes to any step.
- **Why:** agents and scripts edit the project the person has open ([X1](#x1-projects-are-always-saved-standalone-json-files)). Seeing which steps were theirs, and undoing them as one, is what makes that safe.

## X35. The History window docks, and shows a step before going there
- **Resolve:** the History window floats over the page; hovering a row turns it bold, and a click goes there at once. Up and Down don't move in the list (21.1.1).
- **vidcut:** the history docks beside the Media Pool (Edit > History > Open History Window, or History in the top bar), keeping the width it was given between runs, so it covers neither the viewer nor the timeline. Hovering a step, there or in Edit > History, draws the timeline as it was after that step without changing the project, outlines what the step changed, and scrolls the lower timeline there if it is off screen; going to such a step brings the playhead to it, so the timeline stays there. While a past state is drawn, a click on the timeline only ends the preview: it never edits a project the timeline isn't showing. In the panel, Up and Down step through the history the same way, Enter goes there, and Esc comes back to where the project is. A click goes there, as in Resolve. Edit > History lists the newest twenty steps, as in Resolve, until the current step is older than those; then it lists the twenty around the current one.
- **Why:** a window over the picture hides what the history is about, and a label can't show a cut the way the timeline does; looking costs nothing, as the project changes only on a click or Enter. A menu that has scrolled past the current step can't show where the project is, or take it one step either way.

## X36. Nothing is lost: undone steps a new change replaces stay as a version
- **Resolve:** a new change after an undo discards the steps that could be redone (p. 652; checked on 21.1.1).
- **vidcut:** they stay in the history as a version, shown under the step they branched from ("2 undone steps, replaced 5 min ago"), with Restore this version. Restoring brings them back and keeps the steps that had replaced them as a version of their own; a version replaced inside another comes back with it in one step. Hovering a version's step draws the timeline as it was then. `history.get` lists them in `branches`; `history.restore` restores one.
- **Why:** an undo, then an edit by mistake, shouldn't throw away work, least of all an agent's that the person undid to look at what came before.

## X37. Named versions
- **Resolve:** the undo history has no names. Studio 20.3 added named timeline backups (File > Create Timeline Backup…, a "New Timeline Backup" dialog with a Name), copies kept beside the project.
- **vidcut:** Mark This Version… (Edit > History, the History window's Mark… button, or a step's right-click menu) names a step of the history ("Before the silence cut"). The name shows on its step as a flag, Find finds it, and going there is a click; names stay with the history across runs and don't change the project. `history.mark` and `history.unmark` do the same.
- **Why:** the history already holds every version, so a name is all a backup needs here, and naming costs nothing to make or to keep.

## X38. Find, revert one step, and what changed since
- **Resolve:** the History window is a list of labels (21.1.1).
- **vidcut:** the History window finds steps by their words or names, and filters them by kind (trims, deletes, dissolves…) and by who made them, in the versions too. A step's right-click menu has Revert This Step: its change undone alone, as a new step, keeping the steps after it; it is refused, naming the later step, when one changed what it changed (`history.revert`). Hovering a step says what changed since it ("Since this step: 5 clips moved, 2 deleted, 3 dissolves added"), or for a step still to redo what redoing to it brings ("Redoing to here: 1 clip added"), and for a version's step what going there brings ("Restoring to here: …"); `history.get` with `since` (and `branch`) says the same.
- **Why:** in a long history made by several hands, finding the step to go back to and undoing one mistake without losing the edits after it are what people need most.

## X39. A GIF of one frame is a still, whatever its delay
- **Resolve:** a GIF of one frame comes in as a still only when its frame has no delay; with one, it is a video clip as long as the delay (0.1 s came in as 3 frames at 25 fps). A WebP of one frame is a still whatever its frame's duration (checked on 21.1.1).
- **vidcut:** a GIF or WebP of one frame is a still ([D38](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:** the delay is how long a viewer shows the picture, not how long a clip it makes, and a clip of a few frames of one picture is a blink that can't be trimmed longer. GIF and WebP then behave alike.

## X40. Subtitle files are read as they are written
- **Resolve:** Studio 21.1.1, through its scripting API: a Windows-1252 file shows U+FFFD for every accented letter; a WebVTT cue timed without hours is left out; of a TTML file it keeps only the cues of its first region (with none named, it imports nothing) and only the first span of each cue (`Two <span>spans</span> here` becomes "Two spans"); a cue starting before the one before it ends starts at that end instead, and is left out when nothing remains (TOS-HE.srt loses its cue 12). `{\an8}` shows as text; WebVTT `line`, `align` and color classes are ignored.
- **vidcut:** it reads UTF-8, UTF-16 and Windows-1252, WebVTT times without hours, every TTML cue, span and region, and keeps cues that overlap. `{\an8}`, WebVTT `line` and a TTML region's place put a cue at the top, middle or bottom; italic, bold, underline and colors come along from all three formats.
- **Why:** an imported subtitle file must not lose text unnoticed.

## X41. Subtitle files go straight onto a track, placed by their timecode
- **Resolve:** a subtitle file is a Media Pool clip first. Insert Selected Subtitles to Timeline Using Timecode places its cues by timecode and inserts nothing from a file timed from zero; dragging it places its first cue where it is dropped (p. 1230).
- **vidcut:** `subtitles.import` (File > Import Subtitle) puts the cues on a new subtitle track, named after the file, or on one named. A file whose cues all start at or after the timeline's start timecode, or after 01:00:00:00 on a timeline starting at zero and shorter than an hour, in the file's own time base (real time or timecode labels, [D58](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)), is read as timecode: in real time at 23.976 the hour is `01:00:03.600`, where Resolve renders a cue on the first frame of a timeline starting at 01:00:00:00 (`01:00:03.599`); otherwise times count from the timeline's start. `timecode`, `time_base` and `at` override that.
- **Why:** one step, the same from a script, and both kinds of file land where they belong.

## X42. Exported timelines keep their subtitles
- **Resolve:** its OpenTimelineIO, FCPXML, FCP 7 XML, EDL and CSV exports leave the subtitle tracks out (Studio 21.1.1); a subtitle file in an OpenTimelineIO file it imports makes the import fail.
- **vidcut:** an OpenTimelineIO export writes an SRT beside it for each subtitle track with cues, in its timecode, ready for Insert Selected Subtitles to Timeline Using Timecode; a whole timeline's file also holds the cues for vidcut. `export.start` writes subtitle files beside media when asked (`subtitles`).
- **Why:** subtitles made in vidcut should reach the editor that finishes the program.

## X43. A subtitle's text is typed in its own window
- **Resolve:** a selected subtitle's text is typed in the Captions panel of the Inspector, and shows on the clip as it is typed (p. 1233).
- **vidcut:** double-clicking a cue, Enter with one selected, Edit Subtitle… in its right-click menu, or Add Subtitle opens a Subtitle window with its text and position. The text shows on the cue and in the viewer as it is typed, and the position in the viewer as it is picked; both go in as one change on Done (Ctrl+Enter), or when another subtitle is opened in the window. Cancel leaves the cue as it was.
- **Why:** vidcut has no Inspector, and one change per subtitle keeps typing out of the undo history.

## X44. Add Subtitle uses the shown subtitle track
- **Resolve:** each subtitle track's header has a destination control (ST1, ST2, …) that picks the track Add Subtitle puts new subtitles on (p. 1232); Add Subtitle is in a subtitle track's right-click menu (p. 1233).
- **vidcut:** Add Subtitle in Timeline Actions puts the subtitle on the shown subtitle track, making Subtitle 1 when there is none; in a subtitle track's right-click menu it puts it on that track. The header has no destination control.
- **Why:** the track shown in the viewer is the one being worked on, so one control serves both, and Timeline Actions reaches Add Subtitle before there is a track to right-click.

## X45. Subtitles go into any container, and burn in from any track
- **Resolve:** the Deliver page's subtitle Format is a separate file, Burn into video (the selected track) or As embedded captions: CEA-608 in MXF OP1A and QuickTime, or several tracks as subtitle streams or CEA-608 or EAS708 captions in QuickTime and MXF, by the Codec pop-up (Text, CEA-608) (pp. 1247–1248).
- **vidcut:** `subtitles_as` on `export.start`, and Subtitles in the Export dialog: `file`, `stream` or `burn`. A stream is text in every container vidcut writes that holds one: mov_text in MP4, MOV and M4A, SubRip in Matroska, WebVTT in WebM, a stream for each track named (default: the shown track), with its name and language, the shown track's the default stream. `burn` draws one track, the shown one unless another is named ([D61](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)). No CEA-608 or MXF.
- **Why:** text streams are what players read in the containers vidcut writes; CEA-608 is a broadcast format of 32-character lines, and vidcut writes no MXF.

## X46. Create Subtitles from Audio with open models on this computer
- **Resolve:** Studio only, from Timeline > AI Tools on the Edit page, with its built-in AI; more languages come as an Extras download (pp. 1234–1235). Its dialog shows the progress; a caption preset styles the result.
- **vidcut:** `subtitles.transcribe` runs transcribe.cpp on this computer, on the GPU (Vulkan or Metal) unless GPU acceleration is off: Parakeet Ultra (the default: 25 European languages, times per word, small GPUs too), Qwen3-ASR 1.7B (30 languages; until transcribe.cpp runs its aligner, its times are estimates, its text spread over the speech in pieces of 10 s at the most) or Whisper large-v3 (100 languages, times per phrase), or a GGUF file named, with Silero VAD to find the speech. transcribe.cpp's prebuilt library, Silero VAD and the speech models are downloaded into the cache folder only when `download` says so (in the GUI, after a question that gives their size), each checked against its SHA-256. The language, the characters per line, one or two lines and the gap are its options, with Resolve's defaults; `track` and `range` pick where. The answer names the model, the device, the language and how the cues were timed. A GPU with less memory than the model wants gets a warning, never a refusal. No caption presets. In the GUI, Timeline > AI Tools and Timeline Actions open its dialog: the model, with its languages, how it times the words, its size and whether it is on this computer; the GPU memory warning, before the run; the language (Auto, or one the model knows), the range, the tracks it hears (`tracks`: a box for each track with sound, ticked as playback hears it), the track and Resolve's other options, and a line that says which tracks it hears over which range. A model not yet on this computer is downloaded only after a question that gives its size. The run is a job: no progress dialog, Jobs shows its progress, the download's too, and cancels it.
- **Why:** the speech never leaves the computer, any build has it, and a script or agent can run it and choose the model. Parakeet Ultra is the default for its real word times; Qwen3-ASR becomes the English default once it has them. Timeline Actions holds the Cut page's other timeline-wide actions; Jobs keeps long work beside the editing, as exports (X3).

## X47. Ripple Delete Silence can keep only speech
- **Resolve:** Ripple Delete Silence finds silence by sound level: a threshold in dB, pre-head, post-tail and minimum to trip (p. 773).
- **vidcut:** Detect picks Sound level ([D29](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)) or Speech. Speech cuts whatever Silero VAD, as subtitles use it ([X46](#x46-create-subtitles-from-audio-with-open-models-on-this-computer)), doesn't hear as speech, applause and room noise too. Its Threshold is the speech probability (0.5), and Shortest Speech leaves out shorter speech (0.25 s, Silero's own); Pre Head and Post Tail keep 0.15 s around each spoken part, and non-speech shorter than Minimum to Trip (0.5 s) stays. The red stripes, Remove and the one undo step are the same. `detect`, `speech_threshold` and `min_speech` on `edit.ripple_delete_silence` and `analyze.silence`.
- **Why:** a level threshold keeps applause, laughter and room noise louder than it; a speech detector tells them from speech.

## X48. Ripple Delete Silence's stripes can be edited before Remove
- **Resolve:** Ripple Delete Silence shows the parts it will remove as red stripes, and Remove takes them all (p. 773).
- **vidcut:** in the lower timeline, a click on a stripe keeps that part: it turns gray and hatched, and another click cuts it after all. A stripe's edges drag, in whole frames, within its clip and short of the next stripe. Keep All and Cut All do it to every stripe, and the dialog counts what Remove takes ("Removes 160 of 162 parts, …"). While the dialog is open, `]` and `[` (or the two keys right of P) go to the next and previous stripe, X keeps or cuts the one at the playhead, and playing the timeline (the play buttons, Space, J and L) plays it as Remove would leave it, changing nothing; the playhead jumps what Remove closes up, the red stripes on Track 1 or past its end. While the dialog is open, Ctrl+Z and Ctrl+Shift+Z (and Edit > Undo and Redo) undo and redo each change to the stripes and the settings, one at a time: a click, an edge's drag, X, Keep All, Cut All, or a changed setting, which finds the stripes again and whose undo brings the edited stripes back. Remove is still one undo step of the project. Scripts: `parts` on `edit.ripple_delete_silence`, the dry run's `removed` edited.
- **Why:** no threshold gets every pause right: a breath, a pause for effect or a word cut short needs a look and a listen, and fixing them after Remove means undoing it or trimming dozens of clips.

## X49. Subtitles re-time to the speech
- **Resolve:** the Cut page has no way to move a subtitle track to its audio: imported cues keep their file's times (pp. 1230–1231), and Create Subtitles from Audio makes new ones (pp. 1234–1235).
- **vidcut:** Re-time to Speech, in a subtitle track's right-click menu, and Re-time Subtitles to Speech in Timeline Actions for the shown track; `subtitles.retime`. It hears the tracks ticked in its dialog (`tracks`), by default those playback hears. It lines up where the Silero VAD built in for subtitles ([X47](#x47-ripple-delete-silence-can-keep-only-speech)) hears speech with where the cues show: one offset, up to 60 s either way (`max_offset`), and a speed change from 0.95 to 1.05 where one fits better, snapped to 24/23.976, 25/24, 25/23.976 or their inverses when it is within 0.25 s of one over the track. The best offset must score 1.25 times the next best more than 2 s away, or nothing moves and the error gives both. Also match words (`words`) hears the speech with Parakeet Ultra ([X46](#x46-create-subtitles-from-audio-with-open-models-on-this-computer)); a cue matches when its first word is in a run of 3 or more words matching the words heard. With 10 matched cues or more, the first of these to put 90% of them within 0.5 s of their words wins: one offset, an offset and a speed, an offset for each section between jumps, or each matched cue moved to its words, the others as in their section. Find previews it: the offset and speed, how clear the peak was, and with words the cues matched, their errors after the speech's offset and after the correction, and the cues that didn't match. Apply is one undo step; a dry run answers the same. Cues that would end before zero are removed, with a warning.
- **Why:** a subtitle file is often timed for another cut or frame rate of the video. Lining up where speech is, not what is said, works for translations and any language; where the words match, they check it and fix what one offset can't.

## X50. Text-based editing by request, from the timeline's sound
- **Resolve:** Studio transcribes each Media Pool clip, or a timeline, and edits from the Transcription window, on the Cut page too: deleting text leaves its range out, and Timeline Transcription mode cuts the timeline from its clips' transcripts (pp. 994–1009).
- **vidcut:** no Transcription window yet. `analyze.words` hears the timeline's mix and answers with each word and its frames, and `edit.ripple_delete_words` takes those spans out of every unlocked track and closes the gap, ripple on or off ([D63](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)). Nothing is kept: after a cut, the words are heard again. A model without word times is refused.
- **Why:** a script or an agent cuts by the words it is given, and a cut that leaves the hole isn't a cut of those words. Estimates from a model that didn't time the words would cut the wrong frames.

## X51. A subtitle track's style is set in its own window
- **Resolve:** a track's style is set in the Track panel of the Inspector: its font, color, size and position, among others, and a drop shadow, a stroke and a background (p. 1237).
- **vidcut:** double-clicking a subtitle track's name, or Rename Track… in its right-click menu, opens the Subtitle Track window with its name, language and style: the font, typed or picked from the installed ones; the size; the color, the outline and the background, each typed or picked with a swatch; and the margin. The viewer shows the style as it is edited, and it goes in as one `track.set` on Done. Cancel leaves the track as it was. The margin, the inset from the picture's edges, is vidcut's own control where Resolve has Position X and Y.
- **Why:** vidcut has no Inspector ([X43](#x43-a-subtitles-text-is-typed-in-its-own-window)), one change per track keeps typing out of the undo history, and a margin is what subtitle guidelines and IMSC1 regions speak of.

## X52. Add Title and a Title window, without a Titles browser or an Inspector
- **Resolve:** titles are dragged from the Titles browser, which previews them as the pointer hovers its thumbnails, onto the timeline (pp. 762–763, 1176). A title's text is edited by double-clicking it in the Timeline Viewer, where onscreen controls also move, scale and rotate it (pp. 763–764, 1176); its fonts, colors, drop shadows, backgrounds and transform are set in the Video Inspector (p. 764).
- **vidcut:** Add Title, in Timeline Actions and the Timeline menu, puts a 5 s Text title at the playhead (D66) and opens the Title window; double-clicking a title clip, or Edit Title… in its right-click menu, opens it too. The window has the title's kind, a lower third's alignment, its text, its font (typed or picked from the installed ones), size, color, outline and background, each color typed or picked with a swatch, and its place as percents across and down (X54). The viewer shows the title as it is edited, drawn as an export draws it; Done sends it as one `titles.set`, and Cancel leaves the title as it was. `titles.add`, `titles.set` and `titles.list` do the same from a script.
- **Why:** vidcut has no Inspector or effects browser ([X43](#x43-a-subtitles-text-is-typed-in-its-own-window), [X51](#x51-a-subtitle-tracks-style-is-set-in-its-own-window)), one change per title keeps typing out of the undo history, and four kinds need no browser.

## X53. Titles are Text and the three lower thirds, in plain text
- **Resolve:** the Titles category holds L, M and R Lower 3rd, Scroll, Text and Text+, all but Text+ in rich text, and Fusion Titles (p. 1178). A lower third's two lines each have their own rich text and Position, Zoom and Rotation (p. 1178).
- **vidcut:** a title is `text`, one block of text, or a `lower_third` aligned left, middle or right, two lines at most. All its lines share one style. Scroll, Text+ and Fusion titles and rich text are left out ([todo](todo/titles.md)).
- **Why:** a block of text and a lower third are what most cuts need; scrolling and Fusion titles need animation, and words styled apart need a rich-text editor.

## X54. A title's place is a point in fractions of the picture
- **Resolve:** a title's text is moved by dragging it in the viewer, snapping to the frame's center and thirds (pp. 763–764, 1176), or by its Position in the Inspector (p. 764); a lower third sits at a corner or the middle of the bottom of title safe (p. 1178).
- **vidcut:** a title's `position` is `[x, y]` in fractions of the picture's width and height from its top left, percents in the Title window: where a Text title's middle goes, or a lower third's bottom corner or middle (D65). Left out, the title takes its kind's usual place.
- **Why:** fractions mean the same at any picture size and a script sets them exactly, and a lower third hung from its corner stays inside title safe whatever the length of its text.
