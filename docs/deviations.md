# Deviations from the Resolve Cut page

vidcut follows the Cut page ([cut-page-spec.md](cut-page-spec.md)), except where an entry in this file tells otherwise.

Each entry has three parts:

- **Resolve:** what Resolve does, with the page in its manual.
- **vidcut:** what vidcut does.
- **Why:** the reason.

This file also lists the features that vidcut adds. Where the manual does not tell what to do, the spec records a decision ([spec §12](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)). A decision is not a deviation.

## X1. Projects are always-saved standalone JSON files

- **Resolve:**
  - Projects are in a project database. The user saves them, or Live Save saves them (Project Manager chapters).
  - Media that move go offline until the user relinks them (pp. 668–669).
- **vidcut:**
  - A project is one `.vidcut` JSON file.
  - vidcut writes the file atomically after each committed change.
  - The file stores the paths of media in or under the project's folder relative to that folder. Thus the user can move the folder, also to another computer.
  - Relink Media finds missing files by name. It relinks only the same file ([design/model.md](design/model.md#fingerprints-and-relinking)).
- **Why:**
  - Scripts and agents edit projects through the CLI while the GUI can have them open. One file that is always current keeps all of them in agreement.
  - JSON is easy to read and to diff.

## X2. One timeline per project file

- **Resolve:**
  - A project holds many timelines. The Master bin lists them (pp. 655, 678).
- **vidcut:**
  - A project file holds exactly one timeline.
- **Why:**
  - This is simple for a cutting tool, and it is an obvious mapping for scripts.
  - Several timelines are several project files that share media.

## X3. Export dialog beyond Quick Export

- **Resolve:**
  - The Cut page has only Quick Export (Ch. 39, pp. 788–790).
  - Render settings, individual clips, user presets, Normalize Audio and the Render Queue are on the Deliver page (pp. 790, 4105, 4130).
- **vidcut:**
  - Quick Export works as on the Cut page, with a shorter row: H.265 Master, AV1, ProRes 422 HQ, YouTube 1080p and 2160p, TikTok.
  - The Export dialog adds every codec, container and encoder setting from any preset.
  - It also adds a range, per-clip export, audio-only export, loudness normalization, the live plan, the export as a shell command, and user presets.
  - In a running GUI, exports are jobs. One job encodes at a time ([design/export.md](design/export.md#the-guis-exports), [design/live.md](design/live.md#jobs)).
  - No progress dialog opens.
  - The Jobs button in the top bar counts the running jobs. It glows for a moment when a job starts or ends.
  - Jobs shows the progress of each job. An expanded job also shows its encoder, its decoders and its GPU use.
- **Why:**
  - vidcut has no Deliver page, and the user wants these options.
  - An export runs beside the editing. A dialog over the timeline for each export interrupts that editing.
  - The plan and the command let scripts reproduce GUI exports.
  - A short row is quicker to choose from.
  - AV1 makes the smallest files, and current GPUs encode it.
  - For masters, H.265 replaces H.264.

## X4. Undo history persists across sessions and CLI runs

- **Resolve:**
  - Undo is unlimited in a session. Resolve clears it when the project closes (p. 651).
- **vidcut:**
  - The history is a file in the state folder. Thus it stays after a restart and between separate CLI runs.
  - It keeps up to 131,072 steps. Then the oldest 10% go ([design/requests.md](design/requests.md#undo-history)).
- **Why:**
  - An agent can undo its own edits from a later CLI call.
  - The user can undo the batch of an agent from the GUI.

## X5. Scripting additions (no Cut-page behavior changes)

- **Resolve:**
  - The Cut page has none.
- **vidcut:**
  - `edit insert --at` and `edit overwrite --at` place at an exact frame. Smart Insert inserts only at an edit point (p. 697).
  - The command log shows each action as its request, from the GUI or from an agent. The user can copy each one as a `vidcut rpc` command.
  - Commands for a project that is open in a GUI run inside that GUI. Live-only commands drive the GUI ([cli.md](cli.md), [design/live.md](design/live.md)).
  - `edit copy` returns clips as JSON for `edit paste --clips` into any project. In the GUI, Ctrl+C also puts this JSON on the system clipboard.
  - Overwrites with `--use-marks false` place by position, as a drag does.
  - `--track` can name the next new track ([D27](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - `track set` and `markers set` do what the track header, the mixer and the marker dialog do.
  - Source marks are saved ([D6](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - In a running GUI, `export start --background` returns a job id for `jobs list|get|wait|cancel`.
  - `export plan` writes nothing.
  - Other additions are `presets save|delete` and `gui show|close`.
  - `analyze silence|scenes|black|loudness` and `timeline boring` report silences, shots, black, loudness and the clips of the Boring Detector. They do not edit.
  - `storyboard render` draws every shot into one PNG.
- **Why:**
  - Scripts and agents need frame-exact edits without a playhead.
  - They also need measurements to make a plan.
  - The command log teaches the API.

## X6. Removing a bin keeps its clips

- **Resolve:**
  - Delete Bin deletes the bin, the bins in it and their clips (p. 377).
- **vidcut:**
  - `bins.remove` removes the bin and the bins in it. Their media move up to its parent bin.
  - `media.remove` removes media. It needs `force` when clips use the media.
- **Why:**
  - Bins only organize the pool.
  - When a script or a mistake removes a bin, the media and their clips must stay in the project.

## X7. Timelines start at 00:00:00:00

- **Resolve:**
  - New timelines start at record timecode 01:00:00:00 (p. 678).
- **vidcut:**
  - The default start timecode is 00:00:00:00.
  - New Project and Project Settings offer 00, 01 and 10 hours, or any timecode that the user types (`start_tc`).
  - Timecodes in commands are record timecodes.
- **Why:**
  - With a zero start, timecode, seconds and frame numbers count from one point. Thus `00:01:00:00` and `60s` are the same frame.
  - One hour (programmes) and ten hours (UK broadcast) are the usual other starts.

## X8. The source viewer counts project frames

- **Resolve:**
  - Source Clip and Source Tape show a clip at its own frame rate, with its own source timecode (pp. 680, 688).
- **vidcut:**
  - Source viewers step through project frames. They count from the first frame of the clip.
  - They sample frames as the export samples them ([design/export.md](design/export.md#which-source-frame)). For media at the project rate, these are the frames of the media.
  - Source viewers do not show embedded start timecodes.
- **Why:**
  - All positions, also source positions, are in project frames ([design/model.md](design/model.md#time)).
  - Thus what the user marks is exactly what goes into the timeline and the export.

## X9. OpenTimelineIO interchange

- **Resolve:**
  - Timelines move between applications as EDL, AAF, XML or OTIO files. File > Import Timeline and File > Export do this, off the Cut page (pp. 527–530, 4159–4160).
  - An import makes a new timeline.
  - An `.otioz` bundle unpacks into a folder with the name of the bundle.
- **vidcut:**
  - vidcut uses only OpenTimelineIO, for import and for export, in the shapes that Resolve uses ([D40](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - It reads and writes `.otio` files, `.otioz` bundles and `.otiod` bundle folders ([design/model.md](design/model.md#opentimelineio)).
  - File > Export Timeline is `export start cut.otio`. With Media, the export is a bundle.
  - Marks, faders and the rest of the project go in the metadata of vidcut. Resolve does not keep this metadata.
  - A project holds one timeline (X2). Thus File > Import Timeline (`timeline import`) appends, replaces the timeline, adds one compound clip, or brings in only the media.
  - An OTIO file also opens as a new project.
  - Clip effects, transitions, generators, compound clips and speed changes come in as data. vidcut writes them back while they fit.
  - Dissolves, cross fades ([D50](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)) and speeds ([D48](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)) play.
  - Other transitions play as cuts, and generators play as gaps. Other effects do not play.
  - The Text and lower-third titles of Resolve come in as titles of vidcut. The titles of vidcut go out in the shape that Resolve uses ([D67](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - The Transform, Cropping and Retime and Scaling of Resolve come in as the picture of a clip. The picture of a clip goes out as these effects ([D75](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - Resolve Studio 21.1.1 imports `.otio` and `.otioz` files. It imports an `.otiod` folder by its `content.otio`.
  - It keeps tracks and their names, J/L cuts, clip gain, markers, stills, image sequences and the timecode of sources.
  - It also keeps clip effects and their keyframes, transitions, generators, speeds, and compound clips with their picture and sound linked.
  - The OTIO of Resolve leaves out ResolveFX plugins. Thus these plugins never come over.
  - OTIO has no drop-frame flag. An imported timeline takes the frame rate and the drop-frame setting of the project.
  - An import into a project with no media turns drop-frame off.
- **Why:**
  - OTIO is an open standard that Resolve and other editors read and write. Thus a cut can go to a finishing tool and come back.
  - Agents write timelines in a documented format.
  - The EDL of Resolve keeps one video track, and no audio tracks or markers. Its FCPXML (1.10) keeps no markers.
  - vidcut carries what it does not model. Thus a cut that is finished in Resolve can come back for a recut and go back intact.
  - vidcut keeps only the effects in use. Resolve fills in the rest.
  - When an edit removes the cut of a transition, vidcut drops the transition and does not move it. A dissolve between two other shots is a different edit.
  - The media of a bundle get their own folder. Thus bundles unpacked side by side do not mix, and the user can move or delete each bundle as one.

## X10. An MCP server for AI agents

- **Resolve:**
  - The Cut page has none.
  - Resolve Studio has a Python and Lua scripting API (Workspace > Scripts).
- **vidcut:**
  - `vidcut mcp` serves the Model Context Protocol on stdin and stdout.
  - Each request method is a tool ([design/requests.md](design/requests.md), [cli.md](cli.md)).
- **Why:**
  - Agents use MCP to find and call tools.
  - vidcut makes the tools from the request schema. Thus the tools stay complete and the same as the CLI.

## X11. Dynamic Trim and Ripple Delete Silence, simplified

- **Resolve:**
  - A Dynamic Trim run changes the project while it runs. Each run is one undo step (pp. 730–731).
  - Ripple Delete Silence can crossfade the remaining audio (p. 773).
- **vidcut:**
  - A Dynamic Trim run plays while it runs, and the timeline and the viewer show it.
  - The project changes once, when the run stops ([D32](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - Ripple Delete Silence has no crossfade. Its cuts get the 3 ms de-click fades that every cut gets.
  - It can also cut what is not speech ([X47](#x47-ripple-delete-silence-can-keep-only-speech)).
  - The user can edit its stripes before Remove ([X48](#x48-ripple-delete-silences-stripes-can-be-edited-before-remove)).
- **Why:**
  - One change at the end keeps edits at many times a second out of the file, the history and other clients.
  - It is one undo step, the same as in Resolve.
  - The de-click fades already keep the cuts silent.

## X12. Preferences hold only what vidcut needs

- **Resolve:**
  - Preferences has System and User panels with many settings (p. 94).
  - The project settings and the preferences set up caches and proxies (pp. 191–218).
- **vidcut:**
  - File > Preferences (Ctrl+,) holds the settings of `preferences.toml` ([design/model.md](design/model.md#preferences)).
  - These are the theme, GPU use, and the memory and the audio device of the player.
  - They also include the limit of the disk cache, the thumbnails, the proxy rules and folder, and the filler words.
  - `preferences get|set|reset` give scripts access to them.
  - A sidebar selects one page at a time:
    - General: the theme, the preferences file and Reset vidcut…
    - GPU
    - Memory
    - Sound
    - Cache: the kept speech models and paid replies, the size limit of all other contents of the cache, and the thumbnails
    - Proxies
    - Services: the keys of the paid services, and whether to ask before vidcut sends audio
    - Filler Words
  - When vidcut cannot read the file, the window shows why, with Show in Folder and Reset Preferences.
  - Reset Preferences restores the defaults. It keeps the old file as `preferences.toml.bak`, or `.bak.2` and higher. It never replaces a backup.
  - Reset vidcut… on the General page, and `vidcut reset`, restore the state of a fresh install.
  - They reset the preferences, the caches and the recent projects. They also reset the undo histories, except those of open projects.
  - They also reset the view settings, the size and place of the window, and the remembered folders.
  - They reset presets and themes only when the user asks. They never reset a proxy folder that the user set.
  - They do not remove the kept speech models and paid replies while Keep downloaded models and paid replies is on. This setting is on by default.
  - A confirmation comes first. Esc cancels it.
  - Reset vidcut refuses while another vidcut GUI runs, because that GUI would write its settings back.
- **Why:**
  - A preference is here only when vidcut has a behavior to choose.
  - The user can edit a plain TOML file by hand and keep it in dotfiles. Agents can change it.
  - A reset gives a clean start in one step, also for tests. The user does not have to find each file.

## X13. Deinterlacing has one quality

- **Resolve:**
  - Each interlaced clip has a Deinterlace Quality (Normal, High, Neural Engine) in the Inspector.
  - Playback always uses Normal (p. 741).
  - Clip Attributes turn deinterlacing on or off for each clip (p. 451).
- **vidcut:**
  - vidcut always deinterlaces frames flagged as interlaced with `bwdif`, in the preview and in the export ([D34](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - [design/gpu.md](design/gpu.md#deinterlacing-and-hdr) tells how the GPU does it.
- **Why:**
  - One method keeps the preview and the export frame-exact.
  - `bwdif` is the better of the CPU deinterlacers of FFmpeg.
  - There is no Inspector for a choice for each clip.

## X14. Color is set per export, not managed

- **Resolve:**
  - Color management gives the timeline a color space.
  - Media come in through input transforms and tone mapping.
  - The user selects the output space for delivery (pp. 230–239).
- **vidcut:**
  - There is no timeline color space and no grading.
  - Each export selects SDR, PQ or HLG from the footage that it shows, unless the request tells otherwise ([D44](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - It converts each clip to that output from the tags of the clip. Where the tags are wrong, it uses the Input Color Space of the media ([D35](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - The viewer uses the same math.
- **Why:**
  - vidcut cuts. Grading belongs in other tools.
  - Mixed phone, camera and screen footage comes out correctly without color settings.
  - The HDR video of a phone stays HDR.

## X15. The Smart Indicator holds still

- **Resolve:**
  - After a few idle seconds, the Smart Indicator moves up and down on the ruler.
  - The edit point strobes green, or red when a clip reaches its start or end frame (p. 696).
- **vidcut:**
  - The indicator is green. It is red where a clip at the edit shows the first or last frame of its media.
  - It does not move or strobe.
- **Why:**
  - An idle animation repaints the window all the time. On a laptop, this costs CPU time and battery.
  - The color gives the same information.

## X16. The still duration is a project setting

- **Resolve:**
  - The standard still duration is a user preference (Preferences > User > Editing, p. 114).
- **vidcut:**
  - It is a project setting (`project set --still-duration 3s`; [D38](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - An edit is the same in every run on every computer.
  - An agent that appends a still gets the duration that the project sets.

## X17. Image sequences are asked for, not guessed

- **Resolve:**
  - The Frame Display Mode decides whether numbered images are one sequence or stills.
  - Auto decides by file type (p. 366).
- **vidcut:**
  - Each image is a still, unless `media add --sequence` asks for the numbered run around it.
  - `media add-folder --sequence` asks for the runs in a folder ([D39](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - When the user imports numbered images in the GUI, picked or in a folder, vidcut asks which they are.
  - For frame formats (PNG, TIFF, EXR, DPX, TGA, BMP), the question has one clip for each run already selected.
- **Why:**
  - The file type does not tell the frames of a render from numbered photos.
  - A wrong guess turns a slideshow into a blink, or a render into thousands of stills.
  - A flag keeps scripts predictable.

## X18. Proxies keep source timestamps, and only the preview uses them

- **Resolve:**
  - The user generates proxies from the Media Pool.
  - Playback switches between the proxies and the originals. Export reads the originals (pp. 207–218).
  - The manual does not tell whether proxy frames keep the timestamps of their sources.
- **vidcut:**
  - Proxy frames keep the timestamps of their sources. Thus the preview picks the same frames.
  - Only the pictures of the preview use proxies.
  - The sound of the preview, the export, frames, analysis and OTIO read the originals ([D41](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - The preview stays frame-exact with the export. This is also true for image sequences that are too large to decode in real time.
  - The preview cannot drift from the mix or from the delivered frames.

## X19. HDR metadata comes from the sources

- **Resolve:**
  - An HDR export carries the metadata of the grading setup: the Mastering Display and the HDR10 metadata (p. 4117).
  - It also carries Dolby Vision and HDR10+ from the grade (pp. 151, 259–260).
  - Resolve Studio measures MaxCLL and MaxFALL (p. 282).
- **vidcut:**
  - There is no grade. Thus an export carries what its sources tell, or measures it from their frames ([D46](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - Phone, camera and graded HDR footage looks on TVs as its sources do.
  - This needs no setup by a colorist.

## X20. Stats for nerds

- **Resolve:**
  - None. The viewer shows nothing about how it plays.
- **vidcut:**
  - The right-click menu of the viewer opens a panel over the picture.
  - The panel has a group for each of these: decoding, the picture, the viewer, playback, the kept pictures and the sound.
  - `player stats` shows or hides the panel.
  - `player state` has the same facts as `stats`.
- **Why:**
  - Where the GPU decodes and draws is different on different machines.
  - The panel shows it without a debug log.

## X21. Ctrl+wheel zooms the timeline

- **Resolve:**
  - Option+wheel zooms the timeline, and Command+wheel moves through it (Edit page, p. 853).
  - The lower timeline of the Cut page also zooms with Option+wheel.
- **vidcut:**
  - Ctrl+wheel (Cmd on a Mac) and a pinch on a trackpad zoom the lower timeline.
- **Why:**
  - Ctrl+wheel and Cmd+wheel zoom in browsers and most programs.
  - egui gives them and a pinch as one zoom.

## X22. Themes

- **Resolve:**
  - None. Resolve has one dark gray look.
- **vidcut:**
  - vidcut has ten built-in themes. The user can add themes as TOML files.
  - Native, the default, follows the light or dark mode and the accent color of the computer.
  - Omarchy follows the current theme of that desktop.
  - Classic is a flat dark gray, as on the Cut page.
  - Preferences or `preferences set appearance.theme=…` change the theme at once.
- **Why:**
  - Users expect a desktop app to offer a few looks for long sessions.
  - Each theme keeps the layout of the Cut page and the meanings of its colors.

## X23. A Keyboard Shortcuts window

- **Resolve:**
  - The Cut page has none.
  - The Keyboard Customization window lists and changes keys, without explanations.
- **vidcut:**
  - F1, or Help in the top bar, lists every shortcut, with one action in each row.
  - A ? beside a less obvious action explains it in a few plain words.
  - The user cannot change keys.
- **Why:**
  - New users learn the keys and the ideas behind them in one place, in the app.

## X24. Cross fades follow their curves exactly

- **Resolve:**
  - Resolve Studio 21.1.1 renders the curve of a Cross Fade as straight lines between points on the curve.
  - The points are the first sample, every 1000th sample from there and from the edit, and the last sample (at 48 kHz).
  - Early in a +3 dB fade, the gain is up to 0.06 off.
- **vidcut:**
  - The gain of each sample is the value of the curve ([D50](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - The curve gives the transition its name. The curve needs no points from an editor.
  - The two differ by more than 1% only in the 20 ms where a curve leaves silence or reaches it.

## X25. Small additions to the layout

- **Resolve:**
  - The Import Media and Import Media Folder buttons are in the Media Pool (p. 654).
  - On the Cut page, Import Media Folder has no menu item (Resolve Studio 21.1.1).
  - The transport bar has no Clear.
  - Timeline Options (p. 645) have no filmstrip or waveform toggles.
  - Snap is in Timeline Options, not among the timeline buttons (p. 649).
  - The pool does not sort by duration.
- **vidcut:**
  - Import and Import Folder are in the top bar.
  - Import Media Folder… is in the File menu and in the right-click menu of the pool.
  - Clear in the transport bar clears the marks (Alt+X).
  - Timeline Options have Filmstrips and Waveforms.
  - A Snap button is among the timeline buttons. Snap is also in Timeline Options.
  - Duration is one of the sorts of the pool.
  - File dialogs for media, projects and exports each start in the folder that the user last used for them.
- **Why:**
  - Each addition puts one click on something that the Cut page hides in a menu or leaves out.
  - Or it adds a menu item where the Cut page has only a button.
  - No addition changes how an edit works.

## X26. The proxy folder is a preference

- **Resolve:**
  - Proxies go to the "Proxy generation location" of the project (Project Settings, Working Folders, p. 145).
  - They go in subfolders by the folders of their sources (p. 208).
  - Preferences can put them in a "Proxy" subfolder beside each source instead, or ask for a folder each time (p. 98).
- **vidcut:**
  - One preference, `proxies.folder`, sets the folder.
  - The default is `proxies` in the project's folder.
  - A relative folder is in the project's folder. All projects share an absolute folder.
  - The name of a proxy comes from the content fingerprint, the size and the modification time of its source.
  - The project names the proxy by its file alone.
  - Thus projects that use the same file find and play one proxy. Each computer finds the proxy in its own folder.
- **Why:**
  - The location of proxies depends on the disks of the computer, not on the project.
  - A project on a network drive opens on several computers. A folder saved in the project would name the disk of one computer.
  - One setting serves the two reasons to move proxies.
  - The first reason is to keep proxies on a local disk while projects and media are on a network drive.
  - The second reason is to make one proxy for intros and outros that several projects use.

## X27. A dissolve short of media is shortened

- **Resolve:**
  - Sometimes the clips do not have media for the full second at the edit. Then the Cut page of Resolve Studio 21.1.1 adds no dissolve and shows no message.
  - With no media past the cut, Ctrl+T on one selected side adds the full second. The missing frames are black.
  - The manual says that a transition is as long as the media of the clips allow (p. 1159).
  - On the Edit page, Ctrl+T asks whether to trim the clips or skip the edit (p. 1160).
- **vidcut:**
  - vidcut shortens the dissolve to the media that each side has, as the manual says ([D50](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - When neither side has media past the cut, the Dissolve button and Ctrl+T tell this and add nothing.
- **Why:**
  - A key that does nothing without a message makes the user guess.
  - A dissolve through black is a different transition from the one that the user asked for.

## X28. Up/Down in Dynamic Trim move the trim

- **Resolve:**
  - On the Cut page (21.1.1), Up and Down in Dynamic Trim move only the playhead.
  - The selected edit stays. The next J or L moves the playhead back to it.
  - Down during a run rolls the edit to the next edit with the playhead, and removes the clip between them.
  - The manual says that Up and Down move both the selection and the playhead (p. 731).
- **vidcut:**
  - vidcut does what the manual says.
  - Up and Down stop a run and apply it. Then they select the next edit on the same track and move the playhead to it.
- **Why:**
  - Then the keyboard selects the next edit to trim, as the manual tells.
  - No clip disappears.

## X29. A Dynamic Trim run says why it stopped

- **Resolve:**
  - A run that reaches the last frame of a neighbor clip or the end of the media just stops.
  - The Trim Editor comes back with green handles, and nothing tells why (21.1.1).
- **vidcut:**
  - The run stops there and applies.
  - The cut becomes red, and the viewer tells why until the next trim ([D32](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - The message is in the line of the Trim Editor or under the picture.
- **Why:**
  - As in [X27](#x27-a-dissolve-short-of-media-is-shortened), a stop without a message makes the user guess.
  - The user cannot tell whether a key press was missed or the media ran out.

## X30. Slowed sound keeps its pitch on every system

- **Resolve:**
  - The manual says that K held with L plays forward slowly with pitch-corrected sound on macOS (p. 871). Thus on other systems, the slowed sound is lower.
  - K held with J plays backward with slowed sound.
  - Resolve Studio 21.1.1 on Windows plays the sound slowed. The pitch of that sound is not checked.
- **vidcut:**
  - Sound at any speed other than 1× keeps its pitch on every system ([D54](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - This includes slow play and playback backward.
- **Why:**
  - The behavior is the same on all systems.
  - Slowed speech at its own pitch is easy to follow when the user listens for a cut.

## X31. A file is in the pool once

- **Resolve:**
  - When the user imports a file into another bin, Resolve adds a second item for it, with its own media id.
  - When the user imports the file into the bin that holds it, Resolve adds nothing.
  - These results are from Resolve Studio 21.1.1, through its scripting API.
- **vidcut:**
  - A file, or an image sequence, is one pool item.
  - When the user imports it again, it keeps its id.
  - Import Media Folder, or an import into a bin, moves it into that bin ([D53](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - Scripts, the command log and OTIO files name media by id.
  - Two items for one file would split its marks, color and proxy.
  - An agent that imports a folder again would double the pool.

## X32. Import Media Folder names what it left out

- **Resolve:**
  - Import Media Folder leaves out a file that it cannot read, and shows no message. Examples are a text file, or a file that only looks like a video.
  - A folder without media imports nothing.
  - These results are from Resolve Studio 21.1.1, through its scripting API.
- **vidcut:**
  - The result names each skipped file and why vidcut skipped it.
  - The notice in the GUI counts the skipped files, and names up to five of each kind.
  - A folder without media is an error ([D53](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - The reason is the same as in [X29](#x29-a-dynamic-trim-run-says-why-it-stopped).
  - An agent can cut a card or a render folder that the user imports. No clip of it must go missing without a message.

## X33. Each undo step says what, where and how much

- **Resolve:**
  - Resolve Studio 21.1.1 names a step by its kind: "Split Clips", "Remove clips", "Add Transition to Edit Points".
  - Only Append names the media.
  - Edit > Undo and Redo do not tell what they would undo or redo.
  - Ctrl+Z shows nothing.
- **vidcut:**
  - A step tells what it did and by how much.
  - It names the clip by its name, else by the name of its media.
  - It gives the place as a timecode and a track.
  - An example: "Trim the end of Interview.mp4 by -12 frames at 00:01:02:10 on V1".
  - An `apply` batch lists its commands under it.
  - Edit > Undo and Redo name the step.
  - Ctrl+Z and Ctrl+Shift+Z tell what they undid or redid. For a short time, they also outline where it was in the timeline ([D55](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - Ten lines of "Split Clips" do not tell the user which step to go back to.
  - An undo can change something off screen, or something that an agent did. Then it looks as if nothing occurred.

## X34. The undo history says who made each step, and when

- **Resolve:**
  - One editor makes every step.
  - The History window lists only labels (21.1.1).
- **vidcut:**
  - Each step keeps who made it, and when.
  - The maker can be the GUI, the CLI, or an MCP client by its own name.
  - The maker can also be another program over IPC, or a program that wrote the project file.
  - The History window puts a badge on the steps that the GUI did not make.
  - It tells how long ago each step was made.
  - The history lasts longer than one session ([X4](#x4-undo-history-persists-across-sessions-and-cli-runs)). Thus the window marks where each GUI session started ("Opened Tue 14:02").
  - Steps that one client made in a sequence fold into one row, for example the forty edits of an agent.
  - The row opens to show the steps. Its Undo all button goes back to before the first step.
  - A script or an agent on the CLI names itself with `--client` (`VIDCUT_CLIENT`).
  - `history.get` gives all of this. `history.go` goes to any step.
- **Why:**
  - Agents and scripts edit the project that the user has open ([X1](#x1-projects-are-always-saved-standalone-json-files)).
  - The user can see which steps were theirs and undo them as one. This makes it safe.

## X35. The History window docks, and shows a step before going there

- **Resolve:**
  - The History window floats over the page.
  - When the pointer is on a row, the row becomes bold. A click goes to that step at once.
  - Up and Down do not move in the list (21.1.1).
- **vidcut:**
  - The history docks beside the Media Pool (Edit > History > Open History Window, or History in the top bar).
  - Thus it covers neither the viewer nor the timeline.
  - It keeps the width that the user gave it from one run to the next.
  - When the pointer is on a step, the timeline shows the state after that step. This works in the panel and in Edit > History.
  - The project does not change.
  - The timeline also outlines what the step changed. If that part is off screen, the lower timeline scrolls to it.
  - When the user goes to such a step, the playhead moves to it. Thus the timeline stays there.
  - While a past state shows, a click on the timeline only ends the preview. It never edits a project that the timeline does not show.
  - In the panel, Up and Down step through the history in the same way.
  - Enter goes to the step. Esc comes back to the current state of the project.
  - A click goes to the step, as in Resolve.
  - Edit > History lists the newest twenty steps, as in Resolve.
  - When the current step is older than those, Edit > History lists the twenty steps around the current step.
- **Why:**
  - A window over the picture hides what the history is about.
  - A label cannot show a cut as the timeline shows it.
  - A look costs nothing, because the project changes only on a click or Enter.
  - A menu that has scrolled past the current step cannot show the current state of the project.
  - Such a menu also cannot move the project one step in either direction.

## X36. Nothing is lost: undone steps a new change replaces stay as a version

- **Resolve:**
  - A new change after an undo discards the steps that the user could redo (p. 652; checked on 21.1.1).
- **vidcut:**
  - These steps stay in the history as a version.
  - The version shows under the step that it branched from ("2 undone steps, replaced 5 min ago"), with Restore this version.
  - Restore brings the steps back. It keeps the steps that replaced them as a version of their own.
  - A version replaced inside another version comes back with it in one step.
  - When the pointer is on a step of a version, the timeline shows the state at that step.
  - `history.get` lists the versions in `branches`. `history.restore` restores one.
- **Why:**
  - An undo followed by an edit by mistake must not throw away work.
  - This is most important for the work of an agent that the user undid to look at an earlier state.

## X37. Named versions

- **Resolve:**
  - The undo history has no names.
  - Resolve Studio 20.3 and later have named timeline backups. The backups are copies beside the project.
  - File > Create Timeline Backup… opens a "New Timeline Backup" dialog with a Name.
- **vidcut:**
  - Mark This Version… gives a name to a step of the history ("Before the silence cut").
  - It is in Edit > History, on the Mark… button of the History window, and in the right-click menu of a step.
  - The name shows on its step as a flag. Find finds it, and a click goes to it.
  - Names stay with the history across runs. They do not change the project.
  - `history.mark` and `history.unmark` do the same.
- **Why:**
  - The history already holds every version. Thus a name is all that a backup needs here.
  - A name costs nothing to make or to keep.

## X38. Find, revert one step, and what changed since

- **Resolve:**
  - The History window is a list of labels (21.1.1).
- **vidcut:**
  - The History window finds steps by their words or names.
  - It filters steps by kind (trims, deletes, dissolves…) and by who made them, also in the versions.
  - The right-click menu of a step has Revert This Step (`history.revert`).
  - Revert This Step undoes only the change of that step, as a new step. It keeps the steps after it.
  - It refuses when a later step changed what the step changed. The refusal names the later step.
  - When the pointer is on a step, the window tells what changed since that step. An example: "Since this step: 5 clips moved, 2 deleted, 3 dissolves added".
  - For a step still to redo, it tells what a redo to that step brings ("Redoing to here: 1 clip added").
  - For a step of a version, it tells what a move there brings ("Restoring to here: …").
  - `history.get` with `since` (and `branch`) gives the same information.
- **Why:**
  - In a long history from several authors, the user needs two things most.
  - The user must find the step to go back to.
  - The user must undo one mistake and keep the edits after it.

## X39. A GIF of one frame is a still, whatever its delay

- **Resolve:**
  - A GIF of one frame comes in as a still only when its frame has no delay.
  - With a delay, it is a video clip as long as the delay. For example, 0.1 s came in as 3 frames at 25 fps.
  - A WebP of one frame is a still, whatever the duration of its frame.
  - These results are from Resolve Studio 21.1.1.
- **vidcut:**
  - A GIF or WebP of one frame is a still ([D38](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - The delay is how long a viewer shows the picture. It is not the length of a clip.
  - A clip of a few frames of one picture is a blink that the user cannot trim longer.
  - Thus GIF and WebP behave the same.

## X40. Subtitle files are read as they are written

- **Resolve:**
  - These results are from Resolve Studio 21.1.1, through its scripting API.
  - A Windows-1252 file shows U+FFFD for each accented letter.
  - Resolve leaves out a WebVTT cue that has no hours in its times.
  - From a TTML file, it keeps only the cues of the first region. With no named region, it imports nothing.
  - From a TTML file, it keeps only the first span of each cue: `Two <span>spans</span> here` becomes "Two spans".
  - A cue that starts before the previous cue ends starts at that end instead.
  - When nothing of such a cue remains, Resolve leaves it out. TOS-HE.srt loses its cue 12.
  - `{\an8}` shows as text.
  - Resolve ignores WebVTT `line`, `align` and color classes.
- **vidcut:**
  - It reads UTF-8, UTF-16 and Windows-1252.
  - It reads WebVTT times without hours, and every TTML cue, span and region.
  - It keeps cues that overlap.
  - `{\an8}`, WebVTT `line` and the place of a TTML region put a cue at the top, middle or bottom.
  - Italic, bold, underline and colors come in from all three formats.
- **Why:**
  - An imported subtitle file must not lose text without a message.

## X41. Subtitle files placed by their timecode, or from the timeline's start

- **Resolve:**
  - A subtitle file is first a Media Pool clip.
  - Insert Selected Subtitles to Timeline Using Timecode places its cues by timecode. From a file timed from zero, it inserts nothing.
  - The user can drag the file onto a subtitle track. Then its first cue goes where the user drops it.
  - A drop on the gray area above the video tracks makes a new subtitle track. There too, the first cue goes where the user drops it (p. 1230).
- **vidcut:**
  - Import Media, Import Media Folder and File > Import Subtitle (`media.add`, `media.add_folder`) bring in SRT, WebVTT and TTML files.
  - These files go into the Media Pool as subtitle items.
  - A subtitle item shows a subtitle badge, the text of its first cue, and how many cues it holds.
  - vidcut does not read `.srtx`.
  - Insert Selected Subtitles to Timeline Using Timecode (`subtitles.import` with `media`) puts the cues of each file on a new subtitle track. The track has the name of the file.
  - vidcut reads a file as timecode when all its cues start at or after the start timecode of the timeline.
  - A second rule is for a timeline that starts at zero and is shorter than an hour. There, vidcut reads a file as timecode when all its cues start after 01:00:00:00.
  - Both rules use the time base of the file: real time or timecode labels ([D58](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - In real time at 23.976, the hour is `01:00:03.600`.
  - Resolve renders a cue on the first frame of a timeline that starts at 01:00:00:00 as `01:00:03.599`.
  - When neither rule applies, times count from the start of the timeline. On a timeline with a start timecode, the GUI tells this.
  - When the user drags a subtitle item onto a subtitle track, its cues go on that track. The first cue goes where the user drops it.
  - When the user drops the item at another place on the timeline, it makes a new subtitle track.
  - `timecode`, `time_base` and `at` override the placement.
  - `subtitles.import` with a `path` does it in one step, without a pool item.
- **Why:**
  - A file timed from zero also goes on the timeline.
  - A subtitle file can only go on a subtitle track. Thus a drop at another place can only mean a new subtitle track.

## X42. Exported timelines keep their subtitles

- **Resolve:**
  - Its OpenTimelineIO, FCPXML, FCP 7 XML, EDL and CSV exports leave out the subtitle tracks (Resolve Studio 21.1.1).
  - A subtitle file in an OpenTimelineIO file that it imports makes the import fail.
- **vidcut:**
  - An OpenTimelineIO export writes an SRT beside it for each subtitle track with cues.
  - The SRT is in the timecode of the timeline, ready for Insert Selected Subtitles to Timeline Using Timecode.
  - The file of a whole timeline also holds the cues for vidcut.
  - `export.start` writes subtitle files beside media when the request asks for them (`subtitles`).
- **Why:**
  - Subtitles made in vidcut must go to the editor that finishes the program.

## X43. A subtitle's text is typed in its own window

- **Resolve:**
  - The user types the text of a selected subtitle in the Captions panel of the Inspector.
  - The text shows on the clip while the user types (p. 1233).
- **vidcut:**
  - A Subtitle window opens with the text and the position of the cue.
  - These open it:
    - a double-click on a cue
    - Enter with one cue selected
    - Edit Subtitle… in the right-click menu of a cue
    - Add Subtitle
  - The text shows on the cue and in the viewer while the user types.
  - The position shows in the viewer when the user selects it.
  - Both go in as one change on Done (Ctrl+Enter), or when the user opens another subtitle in the window.
  - Cancel leaves the cue as it was.
- **Why:**
  - vidcut has no Inspector.
  - One change for each subtitle keeps typing out of the undo history.

## X44. Add Subtitle uses the shown subtitle track

- **Resolve:**
  - The header of each subtitle track has a destination control (ST1, ST2, …).
  - This control selects the track that Add Subtitle puts new subtitles on (p. 1232).
  - Add Subtitle is in the right-click menu of a subtitle track (p. 1233).
- **vidcut:**
  - Add Subtitle in Timeline Actions puts the subtitle on the shown subtitle track. When there is no subtitle track, it makes Subtitle 1.
  - Add Subtitle in the right-click menu of a subtitle track puts the subtitle on that track.
  - The header has no destination control.
- **Why:**
  - The track that the viewer shows is the track that the user works on. Thus one control does both.
  - Timeline Actions gives Add Subtitle before there is a track to right-click.

## X45. Subtitles go into any container, and burn in from any track

- **Resolve:**
  - The subtitle Format on the Deliver page has three choices.
  - They are a separate file, Burn into video (the selected track), and As embedded captions.
  - Embedded captions are CEA-608 in MXF OP1A and QuickTime.
  - Or several tracks go in QuickTime and MXF as subtitle streams, or as CEA-608 or EAS708 captions.
  - The Codec pop-up (Text, CEA-608) selects which (pp. 1247–1248).
- **vidcut:**
  - `subtitles_as` on `export.start`, and Subtitles in the Export dialog, have three values: `file`, `stream` or `burn`.
  - A stream is text in each container that vidcut writes and that can hold text.
  - vidcut writes mov_text in MP4, MOV and M4A, SubRip in Matroska, and WebVTT in WebM.
  - There is a stream for each named track. The default is the shown track.
  - Each stream has the name and the language of its track. The stream of the shown track is the default stream.
  - `burn` draws one track: the shown track, unless the request names another track ([D61](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - vidcut does not write CEA-608 or MXF.
- **Why:**
  - Players read text streams in the containers that vidcut writes.
  - CEA-608 is a broadcast format with lines of 32 characters.
  - vidcut writes no MXF.

## X46. Create Subtitles from Audio with open models on this computer

- **Resolve:**
  - Only Resolve Studio has it, in Timeline > AI Tools on the Edit page, with its built-in AI.
  - More languages come as an Extras download (pp. 1234–1235).
  - Its dialog shows the progress.
  - A caption preset styles the result.
- **vidcut:**
  - `subtitles.transcribe` runs transcribe.cpp on this computer.
  - It runs on the GPU (Vulkan or Metal), unless GPU acceleration is off.
  - Parakeet Ultra is the default model. It has 25 European languages and times for each word. It also runs on small GPUs.
  - Qwen3-ASR 1.7B has 30 languages.
  - transcribe.cpp does not run the aligner of Qwen3-ASR yet. Thus the times of Qwen3-ASR are estimates, and its text is spread over the speech in pieces of 10 s at most.
  - Whisper large-v3 has 100 languages and times for each phrase.
  - The request can also name a GGUF file.
  - `subtitles.transcribe` finds the speech by its sound level, or with Silero VAD.
  - vidcut downloads the prebuilt library of transcribe.cpp, Silero VAD and the speech models into the cache folder.
  - It downloads them only when `download` tells it to. In the GUI, a question that gives their size comes first.
  - vidcut checks each download against its SHA-256.
  - The options are the language, the characters per line, one or two lines, and the gap. Their defaults are the defaults of Resolve.
  - `track` and `range` select where.
  - The answer names the model, the device, the language, the speaker model (`speakers_by`) and how the cues were timed.
  - While it runs, Jobs and `jobs.get` (`details.speech`) name each of these when it is known.
  - A GPU with less memory than the model needs gets a warning, never a refusal.
  - There are no caption presets.
  - In the GUI, Create Subtitles from Audio… opens its dialog. It is in the Timeline menu beside Add Subtitle, and in Timeline Actions.
  - The dialog shows the looks as tiles. Each tile is drawn in its look.
  - The model list gives the languages of each model, how it times the words, and its size or price ([X62](#x62-paid-speech-recognition-asked-before-the-audio-leaves)).
  - The size of a download shows beside the model.
  - The dialog also has Speakers, and the GPU that the run uses. Before the run, it shows the GPU memory warning.
  - The language is Auto, or a language that the model knows.
  - The dialog also has the range, the tracks that it hears (`tracks`) and the track.
  - The tracks have a box for each track with sound. A box is ticked when playback hears that track.
  - Under Advanced are the lines and the characters per line, the gap, where the words come from, and how the speech is found.
  - A model that is not on this computer yet is downloaded only after a question that gives its size.
  - The run is a job. No progress dialog opens.
  - Jobs shows its progress, also the progress of the download, and can cancel it.
- **Why:**
  - The speech never leaves the computer.
  - Every build has it.
  - A script or an agent can run it and select the model.
  - Parakeet Ultra is the default because it gives real word times.
  - When Qwen3-ASR gives real word times, it becomes the English default.
  - Timeline Actions holds the other timeline-wide actions of the Cut page.
  - The Timeline menu has it with the other subtitle actions, not in a submenu of its own.
  - Jobs keeps long work beside the editing, as for exports (X3).

## X47. Ripple Delete Silence can keep only speech

- **Resolve:**
  - Ripple Delete Silence finds silence by sound level: a threshold in dB, pre-head, post-tail and minimum to trip (p. 773).
- **vidcut:**
  - Detect selects Sound level ([D29](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)) or Speech.
  - Speech cuts all that Silero VAD does not hear as speech, also applause and room noise.
  - It uses Silero VAD as subtitles use it ([X46](#x46-create-subtitles-from-audio-with-open-models-on-this-computer)).
  - Its Threshold is the speech probability (0.5).
  - Shortest Speech leaves out shorter speech (0.25 s, the default of Silero).
  - Pre Head and Post Tail keep 0.15 s around each spoken part.
  - Non-speech shorter than Minimum to Trip (0.5 s) stays.
  - The red stripes, Remove and the one undo step are the same as with Sound level.
  - The requests are `detect`, `speech_threshold` and `min_speech` on `edit.ripple_delete_silence` and `analyze.silence`.
- **Why:**
  - A level threshold keeps applause, laughter and room noise that are louder than the threshold.
  - A speech detector tells them apart from speech.

## X48. Ripple Delete Silence's stripes can be edited before Remove

- **Resolve:**
  - Ripple Delete Silence shows the parts that it will remove as red stripes.
  - Remove removes all of them (p. 773).
- **vidcut:**
  - In the lower timeline, a click on a stripe keeps that part. The stripe becomes gray and hatched.
  - Another click cuts the part again.
  - The user can drag the edges of a stripe, in whole frames. An edge stays in its clip and short of the next stripe.
  - Keep All and Cut All do this to all stripes.
  - The dialog counts what Remove takes ("Removes 160 of 162 parts, …").
  - While the dialog is open, these work:
    - `]` and `[` (or the two keys right of P) go to the next and the previous stripe.
    - X keeps or cuts the stripe at the playhead.
    - Playback of the timeline (the play buttons, Space, J and L) plays it as Remove would leave it. It changes nothing.
    - The playhead jumps over what Remove closes up: the red stripes on Track 1 or past its end.
    - Ctrl+Z and Ctrl+Shift+Z undo and redo the changes to the stripes and the settings, one at a time. Edit > Undo and Redo do the same.
  - A change is a click, the drag of an edge, X, Keep All, Cut All, or a changed setting.
  - A changed setting finds the stripes again. Its undo brings back the edited stripes.
  - Remove stays one undo step of the project.
  - Scripts send `parts` on `edit.ripple_delete_silence`: the `removed` of the dry run, edited.
- **Why:**
  - No threshold gets every pause right.
  - A breath, a pause for effect or a word cut short needs a look and a listen.
  - To fix them after Remove, the user must undo it or trim dozens of clips.

## X49. Subtitles re-time to the speech

- **Resolve:**
  - The Cut page cannot move a subtitle track to its audio.
  - Imported cues keep the times of their file (pp. 1230–1231).
  - Create Subtitles from Audio makes new cues (pp. 1234–1235).
- **vidcut:**
  - Re-time to Speech is in the right-click menu of a subtitle track.
  - Re-time Subtitles to Speech in Timeline Actions does the same for the shown track.
  - The request is `subtitles.retime`.
  - It hears the tracks ticked in its dialog (`tracks`). By default, these are the tracks that playback hears.
  - It aligns where Silero VAD hears speech with where the cues show. This is the Silero VAD built in for subtitles ([X47](#x47-ripple-delete-silence-can-keep-only-speech)).
  - It finds one offset, up to 60 s in either direction (`max_offset`).
  - Where a speed change from 0.95 to 1.05 fits better, it also finds that speed.
  - The snap points for the speed are 24/23.976, 25/24, 25/23.976 and their inverses.
  - When the speed is within 0.25 s of one over the track, it snaps to that ratio.
  - The best offset must score 1.25 times the next best offset more than 2 s away. If not, nothing moves, and the error gives both.
  - Also match words (`words`) reads the words of the speech with Parakeet Ultra, as Create Subtitles from Audio does ([X56](#x56-create-subtitles-from-audio-reads-each-clips-file)).
  - A cue matches when its first word is in a run of 3 or more words that match the words heard.
  - With 10 or more matched cues, it tries these corrections in this order:
    - one offset
    - an offset and a speed
    - an offset for each section between jumps
    - each matched cue moved to its words, and the other cues as in their section
  - The first correction that puts 90% of the matched cues within 0.5 s of their words wins.
  - Find shows a preview: the offset, or one offset for each section, the speed and how many cues move.
  - Under Details, Find shows how clear the peak was.
  - With words, Details also shows the matched cues and the cues that did not match.
  - For the matched cues, it shows their errors after the offset of the speech and after the correction.
  - Apply is one undo step. A dry run gives the same answer.
  - vidcut removes the cues that would end before zero, with a warning.
- **Why:**
  - A subtitle file is often timed for another cut or another frame rate of the video.
  - Alignment by where speech is, not by what is said, works for translations and any language.
  - Where the words match, they check the result. They also fix what one offset cannot fix.

## X50. Text-based editing from transcripts kept with the file

- **Resolve:**
  - Resolve Studio transcribes each Media Pool clip, or a timeline. It edits from the Transcription window, also on the Cut page.
  - When the user deletes text, its range is left out of every edit.
  - Timeline Transcription mode cuts the timeline from the transcripts of its clips (pp. 994–1009).
- **vidcut:**
  - Transcribe Audio (`media.transcribe`) keeps the words of a media file with the measurements that vidcut keeps of the file. It does not keep them in the project.
  - The Transcription window follows the viewer. It shows the words of the source clip, or the words of the timeline from its clips.
  - On a source clip, Delete leaves words out (`media.leave_out`).
  - Then the Place on Top, Insert and Append buttons of the window edit only the rest.
  - The edits of the viewer and of the pool still take all of the clip.
  - On the timeline, Delete ripple deletes the words (`edit.ripple_delete_words`), with ripple on or off ([D63](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - Remove Silent Portions ripple deletes every pause. Each cut is at the quiet points around the pause.
  - Delete Filler Words removes the filler words of the transcript's language in one step.
  - This language is the one that the speech model reported, or the one selected beside the button.
  - Edit… beside the button changes the words of a language for the project or for all projects. The Preferences window does this for all projects too.
  - Follow keeps the word under the playhead in view during playback.
  - While the pointer is over the window, Delete, X, Ctrl+A, Ctrl+C and Esc act on the window.
  - `analyze.words` reads the words for media or clips, or hears the mix of the timeline over a range.
  - The window cannot correct a word. It has no Create Subclip, and it puts no marker on a source clip.
  - Speakers are in [X61](#x61-speakers-are-numbered-per-file-or-per-run-and-joined-by-hand).
  - vidcut refuses a model without word times.
- **Why:**
  - A script or an agent cuts by the words that it gets.
  - A cut that leaves the hole is not a cut of those words.
  - When the words are kept with the file, they serve every project that uses the file. The project file also stays small.
  - If vidcut left words out of every edit, it would change what every edit function does.
  - The buttons of the window cover assembly from a transcript.
  - Estimates from a model that did not time the words would cut the wrong frames.

## X51. A subtitle track's style is set in its own window

- **Resolve:**
  - The user sets the style of a track in the Track panel of the Inspector (p. 1237).
  - The style includes the font, color, size and position, and also a drop shadow, a stroke and a background.
- **vidcut:**
  - A double-click on the name of a subtitle track, or Name and Style… in its right-click menu, opens the Subtitle Track window.
  - The window has the name, the language and the style of the track:
    - a Look list ([X59](#x59-looks-for-titles-too-some-built-in))
    - the font, typed or selected from the installed fonts
    - the size
    - the color, the outline, the background and the color of a drop shadow, each typed or picked with a swatch
    - the opacity, offset and blur of the shadow
    - the margin
    - bold, capitals or small letters
    - how the words show as they are spoken
  - The viewer shows the style while the user edits it.
  - The style goes in as one `track.set` on Done. Cancel leaves the track as it was.
  - The margin is the inset from the edges of the picture. Only vidcut has this control. Resolve has Position X and Y instead.
- **Why:**
  - vidcut has no Inspector ([X43](#x43-a-subtitles-text-is-typed-in-its-own-window)).
  - One change for each track keeps typing out of the undo history.
  - Subtitle guidelines and IMSC1 regions speak of a margin.

## X52. Titles come from a Titles tab or Add Title and are edited in a Title window, without an Inspector

- **Resolve:**
  - The user drags titles from the Titles browser onto the timeline.
  - The browser previews a title when the pointer moves over its thumbnail (pp. 762–763, 1176).
  - The user edits the text of a title with a double-click in the Timeline Viewer. There, onscreen controls also move, scale and rotate the title (pp. 763–764, 1176).
  - The Video Inspector sets its fonts, colors, drop shadows, backgrounds and transform (p. 764).
- **vidcut:**
  - A Titles tab is beside the tab of the Media Pool, and it opens in the same place.
  - The tab shows a card for Text, for each lower third and for Scroll. Each card is drawn as an export draws it.
  - The user drags a card onto either timeline.
  - A double-click on a card adds its title at the playhead and opens the Title window.
  - With Hover Scrub Preview on, the pointer over a card shows that title over the frame of the viewer.
  - The card and the preview of a scroll move as the pointer crosses the card.
  - A title clip on the timeline shows its title at its head.
  - Add Title, in Timeline Actions and the Timeline menu, puts a 5 s Text title at the playhead (D66). It opens the Title window.
  - A double-click on a title clip, or Edit Title… in its right-click menu, also opens the window.
  - The window has these controls:
    - a Look list ([X59](#x59-looks-for-titles-too-some-built-in))
    - the kind of the title
    - the alignment of a lower third or a scroll
    - the text
    - the font, typed or selected from the installed fonts
    - the size, the color, the outline, the background and the drop shadow, each color typed or picked with a swatch
    - the place, as percents across and down (X54)
    - the zoom and the rotation
  - The viewer shows the title while the user edits it, drawn as an export draws it.
  - Done sends it as one `titles.set`. Cancel leaves the title as it was.
  - In the viewer, a click on a title selects it, as in Resolve.
  - When the pointer is near a title, the viewer shows its outline, its corners and the handle above it. They work without a click.
  - Over them, the pointer becomes a move, resize or hand cursor.
  - A drag moves the title. Its place point snaps to the middle and the thirds of the picture, and a guide shows.
  - Shift keeps the drag on one axis. Alt turns snapping off.
  - The corners zoom the title, and the pointer shows the zoom.
  - The handle above the title turns it. With Shift, it turns in steps of 15°.
  - The zoom and the turn are about the pivot of the title.
  - A scroll moves only across.
  - The title shows during the drag. One `titles.set` follows on release.
  - A double-click on the title in the viewer opens the window.
  - `titles.add`, `titles.set` and `titles.list` do the same from a script.
- **Why:**
  - vidcut has no Inspector or effects browser ([X43](#x43-a-subtitles-text-is-typed-in-its-own-window), [X51](#x51-a-subtitle-tracks-style-is-set-in-its-own-window)).
  - One change for each title keeps typing out of the undo history.
  - The Titles tab is the same as in Resolve. Its cards show the kinds of title that vidcut has.
  - Handles that show on hover, and a cursor that changes over them, show that the user can drag them.
  - The zoom by the pointer shows when the zoom stops at its limit, 400%.

## X53. Titles are Text, the three lower thirds and Scroll, styled by markup

- **Resolve:**
  - The Titles category holds L, M and R Lower 3rd, Scroll, Text and Text+, and Fusion Titles (p. 1178).
  - The lower thirds, Scroll and Text use rich text. Text+ does not.
  - The two lines of a lower third each have their own rich text, and their own Position, Zoom and Rotation (p. 1178).
- **vidcut:**
  - A title is one of these:
    - `text`: one block of text
    - `lower_third`: aligned left, middle or right, with two lines at most
    - `scroll` (X55)
  - Markup in its text styles words: `<b>`, `<i>`, `<u>` and `<font color face size>`.
  - The B, I, U, Color, Size, Font and Plain buttons of the Title window put this markup around the selected words.
  - A line of a lower third takes its own size in this way.
  - The lines of a title share one zoom and one rotation.
  - vidcut does not have Text+ and Fusion titles, or keyframes.
  - A title clip has no Transform, Cropping, Speed or blend modes.
- **Why:**
  - Most cuts need a block of text, a lower third and credits.
  - Text+ and Fusion titles are Fusion compositions, and vidcut does not have Fusion (spec §1).
  - Resolve writes Text+ to an OTIO file as a `MissingReference` with nothing of the title in it.
  - Thus neither Text+ nor Fusion titles could come from Resolve or go to Resolve.
  - Keyframes and blend modes are out for every clip (spec §1).
  - Other clips have Transform, Crop and Dynamic Zoom ([X65](#x65-transform-and-crop-are-keys-and-viewer-handles-in-fractions-of-the-frame)). A title does not.
  - The place, zoom, rotation and opacity of a title do what Transform and the opacity of Composite would do.
  - A title has no speed. The speed of a scroll comes from the length of its clip.
  - Other clips have Transform and Crop ([X65](#x65-transform-and-crop-are-keys-and-viewer-handles-in-fractions-of-the-frame)). Dynamic Zoom is [to do](todo/clip-transform.md).
  - Markup in the text is what a script writes and reads.
  - The buttons give the window what the rich-text controls of an Inspector give.

## X54. A title's place is a point in fractions of the picture

- **Resolve:**
  - The user moves the text of a title with a drag in the viewer. It snaps to the center and the thirds of the frame (pp. 763–764, 1176).
  - The Position in the Inspector also moves it (p. 764).
  - A lower third is at a corner or at the middle of the bottom of title safe (p. 1178).
- **vidcut:**
  - The `position` of a title is `[x, y]`, in fractions of the width and the height of the picture, from its top left.
  - The Title window shows it as percents.
  - It is where the middle of a Text title goes, or the bottom corner or middle of a lower third (D65).
  - Without a position, the title takes the usual place of its kind.
- **Why:**
  - Fractions mean the same at any picture size, and a script sets them exactly.
  - A lower third hung from its corner stays inside title safe, whatever the length of its text.

## X55. A scroll rises over the whole clip, its lines spaced as other titles'

- **Resolve:**
  - Scroll moves its text from the bottom of the picture to the top. The length of the clip sets the speed (p. 1178).
  - These are measurements in Resolve Studio 21.1.1, through its scripting API, at 720, 1080 and 2160 lines:
    - The middle of the first line is on the bottom edge one frame before the clip starts.
    - The text rises at a steady speed.
    - How far it goes past the height of the frame changes with the text size and the picture size. It is 1.9 to 2.9 times the text size.
    - Thus the text leaves the top before the clip ends.
    - Its lines are 1.5 to 1.9 times the text size apart, by font and size.
    - Its default lines align left, at 1% of the width of the picture.
- **vidcut:**
  - A `scroll` title rises at a steady speed.
  - It starts just below the picture at the first frame of the clip. It ends just above the picture at the last frame.
  - Its lines are 1.31 times their size apart, as in other titles (D65).
  - They align in the middle of the picture, unless its `align` and `position` tell otherwise.
- **Why:**
  - Credits need a scroll that uses the full clip, and that starts and ends clear of the picture.
  - Its speed comes from the length of the clip alone.
  - The distance in Resolve cannot be found from the size of the text and of the picture.

## X56. Create Subtitles from Audio reads each clip's file

- **Resolve:**
  - Create Subtitles from Audio analyzes the speech in the timeline.
  - It "commonly fails" where dialog overlaps on several tracks. The manual says to mute the tracks that are not wanted (p. 1235).
  - A timeline transcription, in the Transcription window, transcribes the media of each clip through its In and Out instead (p. 1001).
- **vidcut:**
  - By default (`hear: media`), `subtitles.transcribe` reads the words of the file of each heard clip.
  - It uses the words kept with the file, from Transcribe Audio or an earlier run. It does this when the same model made them in the requested language.
  - Otherwise it transcribes the whole file and keeps its words.
  - It places the words through the range and the speed of each clip, J and L cuts too.
  - Two tracks can have the same word at the same time, as a lav and a camera microphone do.
  - Then it keeps the word of the first track (V1, V2, …, then A1, A2, …).
  - `hear: mix` hears the mix of the tracks over the range and keeps nothing.
  - `hear: mix` is the default over In–Out or any other part of the timeline. Thus vidcut hears and holds in memory only a part of a long recording.
  - Whisper large-v3, Qwen3-ASR and a model file of the user always use `hear: mix`, because they do not time words.
  - `hear: transcript` takes the words that each file already has, from any model. It transcribes only a file without words.
  - vidcut hears the sound of a compound clip only in the mix.
  - Also match words in Re-time to Speech reads the words in the same way.
  - When a heard file has a transcript, the first row of the dialog, Words, offers From Transcript or Choose Model.
  - From Transcript is selected, unless the user selected Choose Model before.
  - When every heard file has a transcript, From Transcript names the model and the language that made them. It needs no model.
  - Otherwise, From Transcript names the files that it transcribes first, with the selected model.
  - Choose Model uses again what the selected model made before.
  - With Choose Model, or with no transcript, Words From under Advanced selects between the file and the mix.
  - Then the dialog tells how many of the heard files it transcribes first, and how long they are.
  - It also tells how many transcripts from another model it replaces.
- **Why:**
  - After one transcription, the words of a file serve subtitles, the Transcription window and cutting by words.
  - They do this in every project that uses the file, and they agree.
  - Subtitles from an existing transcript take a moment and cost nothing.
  - A file heard alone is clearer than the mix.
  - Two microphones that record the same speech give it once.

## X57. The lower timeline keeps its height, and its tracks scroll

- **Resolve:**
  - The manual does not tell.
  - On the Cut page of Resolve Studio 21, more video tracks make the timeline taller and the viewer smaller. V1 stays at the bottom.
- **vidcut:**
  - The timeline opens as tall as the tracks of the project need, up to 60% of the window.
  - Then the lower timeline keeps the height that the user gives it.
  - Its tracks shrink to fit, to 22 px at least.
  - When they still do not fit, they scroll. The user scrolls them with the mouse wheel over the tracks or their headers, or with the scrollbar at the right edge.
  - The lower timeline opens on the bottom tracks, V1 and the audio.
  - The rulers and the upper timeline do not move.
- **Why:**
  - The viewer stays big when there are many tracks.
  - The user watches a cut in the picture.

## X58. Animated subtitles are a track style, made from the speech in one step

- **Resolve:**
  - AI Animated Subtitles are only in Resolve Studio. They are Fusion title templates in the Animated section of the Titles tab.
  - After the subtitles are made, the user drags a template onto the header of a subtitle track. The user sets it in the Inspector (p. 1240).
  - After a correction, Update Word Timings times the words of a cue again. Synchronize Word Timings does this for a track (p. 1241).
- **vidcut:**
  - The animation is the `words` of a track style, with `bold`, `case` and `shadow`.
  - The user sets them in the Subtitle Track window or with `track.set` (D69).
  - The looks of Create Subtitles from Audio ([X46](#x46-create-subtitles-from-audio-with-open-models-on-this-computer)) put the speech on a new track that is already animated.
  - Word times follow edits and new text automatically (D70).
  - Synchronize Word Timings, in the menu of a subtitle track header, times captions that were not made from the speech.
- **Why:**
  - It is one step from the speech to animated subtitles.
  - The user does not make subtitles, find a template and drop it on the track. Fusion is not necessary.
  - Word times stay correct after an edit or a correction, and there is no button to remember.

## X59. Looks for titles too, some built in

- **Resolve:**
  - The user saves the style of a subtitle track as a preset from the option menu of the Subtitle Inspector.
  - The user can load a preset onto a track, update it and delete it.
  - The user can also export or import it as a `.preset` file (pp. 1239–1240).
  - Titles have no presets. The manual names no built-in presets.
- **vidcut:**
  - Looks are for subtitle tracks and titles ([D71](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - The user selects, saves and deletes looks in the Subtitle Track and Title windows.
  - Apply Look in the header of a subtitle track applies one.
  - The user drags title looks from the Titles tab as cards.
  - Built-in looks come first, and the user cannot change them.
  - There is no export or import.
  - Each look is a TOML file in the config folder. The file copies to another computer as it is.
- **Why:**
  - The user uses a title look again as often as a subtitle track look.
  - A list with good looks in it gives a start.
  - A file for each look, as for export presets, is its own export and import.

## X60. The pool's metadata shows a file's format, and Clip Info shows all of it

- **Resolve:**
  - A tag icon beside the search field of the pool opens the Metadata Display palette.
  - The palette shows the camera name, scene, shot and take, frame rate and resolution of the selected clip (p. 665).
  - The lower-right corner of a thumbnail opens an overlay of its information (p. 353).
  - The File tab of the Inspector shows the clip details and edits the logging fields (p. 412).
- **vidcut:**
  - The tag icon shows a strip at the top of the pool. The strip stays on until the user turns it off.
  - The strip shows the size, frame rate, codec, bits and chroma subsampling, SDR or HDR, and length of the selected clip.
  - It also shows the start timecode, the sound and the file size.
  - Clip Info shows all that vidcut knows of the file.
  - The info button of the strip or of a thumbnail corner opens it. The menu of a pool item and `gui.show clip_info` also open it.
  - It reads the file again for its container and streams: codecs and profiles, bit rates, color tags, channel layouts, languages and tags (`media.details`).
  - Copy puts all of it on the clipboard as text.
- **Why:**
  - vidcut keeps no logging fields (camera, scene, shot, take). Thus the strip shows the format, which every file has.
  - One window with all the information, read fresh from the file, tells the format without a separate tool.

## X61. Speakers are numbered per file or per run, and joined by hand

- **Resolve:**
  - Speaker Detection (Studio) remembers each voice across the clips of a project. Thus it finds a speaker from one clip again in the next clip.
  - Display Speakers shows the speakers. A double-click on a name changes it.
  - Assign To moves text to another speaker.
  - Reset Speaker Data forgets the speakers (pp. 997–1001).
- **vidcut:**
  - On this computer, speakers come numbered for each file.
  - In English and Chinese, MOSS-Transcribe-Diarize numbers them. Otherwise Sortformer numbers them, up to 4.
  - With a paid request, speakers are numbered across all files of the request.
  - The paid requests are those of xAI and of Gemini. The request of Gemini takes up to 30 minutes ([D73](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - The Transcription window starts a paragraph at each person. The name is a menu: Rename, Same Person As and Separate.
  - Same Person As joins the speaker of this file with another person.
  - Assign To gives the selected words to another person.
  - `speakers.list`, `speakers.set`, `speakers.link` and `speakers.assign` do the same by request.
  - vidcut cannot join speakers by their voices yet, in a project or across projects. This waits for transcribe.cpp ([to do](todo/voice-signatures.md)).
- **Why:**
  - No model that vidcut runs gives the signature of a voice yet.
  - The models give numbers for each file or for each request.
  - To join the numbers by hand once in a project is quick.
  - An automatic join that is wrong would be worse than no join.

## X62. Paid speech recognition, asked before the audio leaves

- **Resolve:**
  - Transcription runs on the computer with its own AI (pp. 994–1002, 1234–1235).
  - There is no outside service.
- **vidcut:**
  - Two more models run on the servers of their companies: `grok-voice-transcribe-2.0` of xAI and `gemini-3.5-transcribe` of Google.
  - Under Advanced, each model has those of its options that keep word times (`xai`, `google`).
  - Before vidcut sends audio, the GUI asks. It shows the company, the length, the size and the estimated cost at the published rate.
  - The GUI does not ask when the question for that service is off.
  - Scripts pass `send`.
  - The files of a run go up together in one request.
  - The request of xAI takes up to 500 MB. The request of Gemini takes up to 30 minutes of speech, and vidcut refuses more.
  - vidcut keeps each reply in the cache, named by the company, its settings and the audio.
  - Thus the same run again reads the reply and costs nothing.
  - A key comes from `XAI_API_KEY` or `GEMINI_API_KEY`, or from the keychain of the system (Preferences > Services, `services.set_key`).
  - A key is never in a file that vidcut writes.
  - Local models stay the default.
- **Why:**
  - The paid models hear some languages and voices better than the local models.
  - vidcut asks first and keeps what the user paid for. Thus nothing is sent or charged without warning.
  - A key is never in a file that a backup or a dotfiles repository copies.

## X63. Subtitles keep their speaker, painted in the person's color

- **Resolve:**
  - Speakers show in the Transcription window and in an exported transcript, with names and timecodes (pp. 1000–1002).
  - A subtitle cue has no speaker. Create Subtitles from Audio gives none (pp. 1234–1235).
- **vidcut:**
  - Each cue keeps the person who says it (`speaker`).
  - Create Subtitles from Audio with speakers sets it. Splits, joins and edits keep it.
  - A person can have a color (`speakers.set`).
  - A cue can have its own color (`color`), which overrides the color of the person.
  - The Speakers setting of the subtitle track (`speaker_color`) tells what the color paints.
  - It paints the spoken word on a track that shows words as spoken, else all of the text. Or it paints nothing.
  - In the Subtitle window, every row except the Name of the person changes only that cue.
  - These rows are its Speaker, its Color and its Position.
  - The Speaker list also has All of the person's. This makes the person into another person everywhere (`speakers.join`).
  - This changes their cues, the speakers of the files, and the words that were theirs.
  - Apply to All of the person's (`subtitles.match_speaker`) gives the color of the person to the cue.
  - It also gives the position of the cue to all their cues on the track.
  - The block of a cue on the timeline has a stripe in its color.
  - WebVTT files carry the speaker as a voice (`<v Nikki>`). vidcut reads it back.
- **Why:**
  - Captions show speakers apart by color.
  - A cue that knows its speaker can take that color, without an edit to every cue.
  - A one-off cue keeps its own color.
  - A WebVTT voice is the standard way to name the speaker. Players and caption tools read it.

## X64. Voice Isolation is free, on a clip or a track

- **Resolve:**
  - Voice Isolation is only in the Studio version. It is a plugin on a mono, stereo or dual-mono track.
  - It runs in real time, but not on live input or on more than two channels.
  - Its Amount mixes the source with the isolated voice. At 50, they are about equal. At 70 to 80, it sounds natural (p. 774).
- **vidcut:**
  - The `voice_isolation` of a clip or a track is an Amount from 1 to 100.
  - It runs `dpdfnet2_48khz_hr` of DPDFNet (Apache-2.0) on the CPU of this computer.
  - It runs live in playback, and the same in an export.
  - It works on any number of channels, each channel alone.
  - The Amount of a clip overrides the Amount of its track. A clip at 0 stays off on a track that has it on.
  - Voice Isolation in the right-click menu of a clip, or Alt + V, turns it on or off for the selected clips.
  - On, it sets the Amount to 75, the middle of the 70 to 80 that the manual gives.
  - The strip of the Mixer does it for a track.
  - An Amount field beside each one sets the Amount.
  - The model, 10 MB, is downloaded the first time, after a question (`download` in scripts).
  - vidcut checks the model against its SHA-256.
  - An OTIO file keeps the Amounts in the metadata of vidcut. Its export tells that other editors do not hear them.
- **Why:**
  - After the cut of its silences, a Meet recording needs noise removal more than anything else (use case 2).
  - A model that runs on one CPU thread at a tenth of real time needs no paid version.
  - On a clip, it cleans one noisy recording and does not change the rest of its track.

## X65. Transform and Crop are keys and viewer handles, in fractions of the frame

- **Resolve:**
  - The Tools button of the viewer opens a bar of effects for the current clip.
  - Transform and Crop each have onscreen controls in the viewer and a Reset (pp. 766–768).
  - Each also has a toggle that turns it off and keeps its settings.
  - Crop is in pixels (p. 768).
- **vidcut:**
  - T shows the Transform handles of the selected clip in the viewer. Shift+T shows its Crop handles.
  - The same key hides them. F1 lists both.
  - The Tools button under the viewer shows a row. The row has Transform and Crop, which are lit while their handles show.
  - The row also has Flip Horizontal and Flip Vertical for Transform, and a Reset for each of the two.
  - The right-click menu of the clip has Transform, Crop, Reset Transform and Reset Crop.
  - There is no toggle.
  - The handles are those of a title (X52), with the same snapping.
  - vidcut adds controls that the Cut page does not have. While the Transform handles show, the wheel or a pinch over the picture zooms it.
  - Alt+wheel turns it, and a drag just outside a corner turns it.
  - The place, zoom and crop of a picture are fractions of the frame and of the source ([D75](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - The requests are `picture.get`, `picture.set` and `picture.clear`.
  - `gui.show transform` or `crop` shows the handles. `gui.show tools` shows the row.
  - Dynamic Zoom: Alt+T shows the start box and the end box of the clip in the viewer. Alt+T again hides them.
  - T, Shift+T and Alt+T change between the three tools. F1 lists Alt+T.
  - The Tools row has Dynamic Zoom, lit while its boxes show. With it come Swap, a menu of the four eases and Reset.
  - The right-click menu of the clip has Dynamic Zoom and Reset Dynamic Zoom. `gui.show dynamic_zoom` shows the boxes.
- **Why:**
  - There is a key for everything.
  - The row shows only while Tools is on. Thus the picture keeps its space.
  - Undo puts a picture back, as a toggle would.
  - Image and design editors zoom and turn a layer with the wheel and with a drag outside a corner. The user does not have to aim at a small handle.
  - A sequence of wheel notches is still one undo step.
  - Fractions, as for the place of a title (X54), keep a picture where it is when the size of the timeline changes.
  - A script does not need the sizes to place a picture.

## X66. Files and folders dropped on the window go into the open bin

- **Resolve:**
  - Clips dragged from the file system go into the Media Pool. A drop on a bin in the Bin list puts them in that bin (p. 364).
  - For the Cut, Edit, Fusion and Fairlight pages, the manual says macOS only (p. 372).
  - Folders dragged from the file system into the Bin list come in as bins, nested as they were (p. 365).
  - The manual does not tell what a folder dropped on the Media Pool of the Cut page does.
- **vidcut:**
  - With a project open, files and folders dropped anywhere on the window go into the open bin.
  - The files come in as Import Media brings them in (`media.add`). vidcut asks about numbered images, as Import Media does.
  - Each folder comes in as Import Media Folder brings it in (`media.add_folder`): a bin, with a bin for each folder in it.
  - While they are over the window, the Media Pool shows Drop to Import.
  - With no project open, a drop does nothing, because Import is not available then.
  - Drops work on Windows, macOS and X11. A Wayland window gets no drops ([design](design/live.md#what-the-gui-computes-itself)).
- **Why:**
  - The window does not get the place where files are dropped. Thus a drop cannot aim at a bin or the timeline.
  - The open bin is where Import puts media.
  - A folder keeps its folders as bins, as the Bin list of the manual does.
  - The Cut page has a Bin dropdown, not a Bin list (p. 655).
  - Nothing about a drop needs macOS.

## X67. The eases of Dynamic Zoom are curves

- **Resolve:**
  - The ease of a Dynamic Zoom is Linear, Ease In, Ease Out or Ease In and Out (p. 768).
  - Resolve Studio 21.1.1 keeps the ease and writes it to OTIO files.
  - But it plays all four eases as the same straight line ([gotchas](gotchas.md#resolves-otio-files)).
- **vidcut:** t is the part of the clip that has played, from 0 to 1. The box goes from the start box to the end box as e goes from 0 to 1:
  - Linear: e = t.
  - Ease In: e = t², slow at the start.
  - Ease Out: e = 1 − (1 − t)², slow at the end.
  - Ease In and Out: e = 3t² − 2t³, slow at the two ends.
- **Why:**
  - An ease that changes nothing is of no use.
  - These curves do what the names of the eases tell.
  - In Resolve, a clip with one of these eases plays at one speed.

## X68. Ducking times every track as a mono track

- **Resolve:**
  - The Ducker times a mono target track by its controls (pp. 778–779; [D76](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - On a stereo target track, Rise Time, Hold and Recovery Time act at half their values.
  - These facts come from renders of Resolve Studio 21.1.1. The left channel had a sine and the right channel a cosine, so the gain of each channel is known.
  - The times of the table are for the right channel, in ms. A fall time is from the start of the source. A recovery time is from its end.

  | Duck Amount | Lookahead | Rise Time | Hold | Recovery Time | Fall starts | Full dip | Recovery starts | Recovery ends |
  |---|---|---|---|---|---|---|---|---|
  | 2.7 | 15 | 10 | 150 | 750 | −40 | −30 | +60 | +430 |
  | 10 | 15 | 10 | 150 | 750 | −40 | −26 | +60 | +430 |
  | 10 | 15 | 200 | 150 | 750 | −230 | −130 | −29 | +348 |
  | 10 | 15 | 10 | 500 | 750 | −40 | −26 | +231 | +608 |
  | 10 | 15 | 10 | 150 | 1000 | −40 | −26 | +60 | +556 |
  | 10 | 100 | 10 | 150 | 750 | −120 | −106 | +20 | +390 |

  - The fall starts at the same time as on a mono track.
  - The fall with a Rise Time of 200 ms lasts 100 ms. The recovery lasts 376 ms at 750, and 500 ms at 1000.
  - The recovery starts (Hold − Lookahead − Rise Time) / 2 after the source ends, to within 7 ms.
  - The recovery, and the fall with a Rise Time of 200 ms, are half cosines, as on a mono track.
  - The two channels get different gains while the gain changes.
  - At 10 dB with a Rise Time of 10 ms, the right channel falls 8.2 dB in the first 16 samples. The left channel falls 0.4 dB.
  - In a fall, the gain of each channel jumps at the 10 ms steps.
  - In the recovery, the left channel is 1 to 9 ms later than the right, and 0.2 dB apart at most.
  - The cause of these differences is not known.
- **vidcut:**
  - Every track has the timing of a mono track, whatever its channels.
  - Every channel of the track gets the same gain.
- **Why:**
  - The mono timing matches the controls. A Hold of 150 ms holds the dip for 150 ms.
  - Gains that differ between the channels move the stereo image while the track dips.
  - The cause of the stereo timing is not known. Thus vidcut copies only the timing that matches the controls.

## X69. Ducking follows speech by default, with one fall shape

- **Resolve:**
  - The Ducker follows the level of its sources (pp. 778–779).
  - A fixed threshold, measured between −48 and −36 dBFS peak, makes a source active ([D76](cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
  - With a Rise Time of 10 ms, the fall is a one-pole curve on the linear gain, not a half cosine.
  - In each 16 samples of that fall, the gain closes 7.0 % of the remaining distance. Thus its time constant is 4.6 ms.
  - With a Rise Time of 200 ms, the fall is a half cosine of 202 ms. The fit error is 0.04 dB rms.
  - Each fall starts at a whole multiple of 10 ms from the start of the timeline.
  - A fall starts Lookahead + Rise Time + 10 to 15 ms before the source.
  - The gain changes in steps of 16 samples.
  - A new Ducker on a timeline with two audio tracks has the other track as Source 1. With three tracks, Source 1 is empty.
  - A typed Recovery Time of 2000 ms becomes 1000 ms.
- **vidcut:**
  - Detect selects Speech, the default, or Sound level.
  - Speech: a source is active where Silero VAD hears speech in it, as Ripple Delete Silence finds it ([X47](#x47-ripple-delete-silence-can-keep-only-speech)).
  - Sound level: a source is active where its peak, after its clip gain and fader, is at or above the Threshold.
  - The Threshold is −42 dBFS by default, the middle of the measured range. It is under Advanced.
  - The fall is a half cosine in dB over the Rise Time, for every Rise Time.
  - Each fall starts Lookahead + Rise Time before the source, with no 10 ms steps.
  - The gain changes on each sample.
  - A new ducking has every other track with sound as its sources.
  - `track.set` refuses a value outside its range.
  - Alt + D turns the ducking of the selected clip's track on or off. It is vidcut's own key.
  - A track without ducking then gets the usual values, under the other tracks with sound.
  - The requests are `duck` on `track.set`, with its `detect` and `threshold_db`.
- **Why:**
  - Room noise, breaths, bumps and music on a dialogue track are louder than a fixed threshold. They dip the target where nobody speaks.
  - A speech detector tells speech apart from them. vidcut already detects speech for Ripple Delete Silence and subtitles.
  - Sound level keeps the behavior of the Ducker, for sound effects and music as sources.
  - One fall shape for every Rise Time is easier to predict. The half cosine is the shape of Resolve's fall at 200 ms, and of its recoveries.
  - A start on a grid of 10 ms is of no use to the user.
  - The usual mix ducks music under every other track. With speech as the default, tracks without speech dip nothing.
  - A value that changes without a word is a different setting than the one asked for.
