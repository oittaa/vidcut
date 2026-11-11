# Deviations from Resolve

vidcut follows DaVinci Resolve ([spec.md](spec.md)), except where an entry in this file tells otherwise.

Each entry has three parts:

- **Resolve:** what Resolve does, with the page in its manual.
- **vidcut:** what vidcut does.
- **Why:** the reason.

A feature that vidcut adds, and that Resolve does not have, is not a deviation. The spec describes it. Where the manual does not tell what to do, the spec records a decision ([spec §12](spec.md#12-decisions-on-gaps-and-contradictions)). A decision is not a deviation.

## X1. A project is one file, and its media paths are relative

- **Resolve:**
  - A Project Library is a database that holds one or more projects. The Project Manager creates and loads projects from it (p. 4224).
  - Media that move go offline until the user relinks them (pp. 668–669).
  - Relink Selected Clips matches clips to files by file name and timecode (p. 669).
- **vidcut:**
  - A project is one `.vidcut` JSON file. There is no library of projects.
  - The project file stores the paths of media in or under the project's folder relative to that folder.
  - Relink Media finds missing files by name. It relinks only the same file ([design/model.md](design/model.md#fingerprints-and-relinking)).
- **Why:**
  - Scripts and agents edit projects through the CLI while the GUI can have them open. One file keeps all of them in agreement.
  - JSON is easy to read and to diff.
  - The user can move the project folder, also to another computer, and keep its media.
  - A different file with the same name does not take the place of the media.

## X2. One timeline per project file

- **Resolve:**
  - A project holds many timelines. The Master bin lists them (pp. 655, 678).
- **vidcut:**
  - A project file holds exactly one timeline.
- **Why:**
  - This is simple for a cutting tool, and it is an obvious mapping for scripts.
  - Several timelines are several project files that share media.
  - The owner decided that several timelines in one project can wait.

## X3. Exports are jobs, and Quick Export has a shorter row

- **Resolve:**
  - Quick Export shows a progress bar dialog while it exports (p. 789).
  - Create Subtitles from Audio shows its progress in a dialog box (p. 1235).
  - The Deliver page has the render settings and the Render Queue (pp. 4103–4132).
  - In Resolve Studio 21.2, the Quick Export row is H.264 Master, HyperDeck, H.265 Master, ProRes 422 HQ, YouTube, Vimeo, TikTok, Presentations, Dropbox and Replay.
  - YouTube has a menu of four sizes, 720p to 2160p (p. 4105).
  - HyperDeck makes H.264 files for a Blackmagic HyperDeck (p. 4108). Presentations uploads to Blackmagic Cloud (p. 4106).
- **vidcut:**
  - The Quick Export row is AV1, H.264 Master, H.265 Master, ProRes 422 HQ, YouTube 1080p, YouTube 2160p, Vimeo 1080p and TikTok.
  - It has no HyperDeck, Presentations, Dropbox or Replay.
  - The Export dialog also shows the live plan, and gives the export as a shell command.
  - In a running GUI, exports are jobs ([design/export.md](design/export.md#the-guis-exports), [design/live.md](design/live.md#jobs)).
  - Create Subtitles from Audio is a job too. It downloads its models as a job.
  - No progress dialog opens. Jobs shows the progress of each job, and can cancel it.
- **Why:**
  - The plan and the command let scripts reproduce GUI exports.
  - An export runs beside the editing. A dialog over the timeline for each export interrupts that editing.
  - vidcut does not upload, and it has no settings for a Blackmagic HyperDeck. The Export dialog has H.264 for other needs.
  - One item in the row is one preset, with no menu. Thus YouTube shows as two items, 1080p and 2160p.

## X6. Removing a bin keeps its clips

- **Resolve:**
  - Delete Bin deletes the bin, the bins in it and their clips (p. 377).
  - These are measurements in Resolve Studio 21.2:
    - Remove Selected Clip on media that timeline clips use asks no question. The pool item goes.
    - The clips of that media on the timeline stay, offline.
    - A compound clip that holds a clip of that media stays in the pool and on the timeline.
- **vidcut:**
  - `bins.remove` removes the bin and the bins in it. Their media move up to its parent bin.
  - `media.remove` removes media. It needs `force` when clips or compound clips use the media.
  - `force` also deletes those compound clips, on the timeline and in the pool.
- **Why:**
  - Bins only organize the pool.
  - When a script or a mistake removes a bin, the media and their clips must stay in the project.

## X8. The source viewer counts project frames

- **Resolve:**
  - Source Clip and Source Tape show a clip at its own frame rate, with its own source timecode (pp. 680, 688).
- **vidcut:**
  - Source viewers step through project frames. They count from the first frame of the clip.
  - They sample frames as the export samples them ([design/export.md](design/export.md#which-source-frame)). For media at the project rate, these are the frames of the media.
  - Source Timecode adds the start timecode of the file, as project frames ([spec D91](spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - All positions, also source positions, are in project frames ([design/model.md](design/model.md#time)).
  - Thus what the user marks is exactly what goes into the timeline and the export.

## X9. OpenTimelineIO interchange

- **Resolve:**
  - Timelines move between applications as EDL, AAF, XML or OTIO files. File > Import Timeline and File > Export do this, off the Cut page (pp. 527–530, 4159–4160).
  - An import makes a new timeline.
- **vidcut:**
  - vidcut uses only OpenTimelineIO, for import and for export, in the shapes that Resolve uses ([D40](spec.md#12-decisions-on-gaps-and-contradictions)).
  - File > Export Timeline is `export start cut.otio`. With Media, the export is a bundle ([design/model.md](design/model.md#opentimelineio)).
  - A project holds one timeline (X2). Thus File > Import Timeline (`timeline import`) appends, replaces the timeline, adds one compound clip, or brings in only the media.
  - An OTIO file also opens as a new project.
  - OTIO has no drop-frame flag. An imported timeline takes the frame rate and the drop-frame setting of the project.
  - An import into a project with no media turns drop-frame off.
  - Clip effects, transitions, generators that vidcut does not draw, compound clips and speed changes come in as data. vidcut writes them back while they fit.
  - Dissolves, cross fades ([D50](spec.md#12-decisions-on-gaps-and-contradictions)), wipes and speeds ([D48](spec.md#12-decisions-on-gaps-and-contradictions)) play.
  - The generators of [spec §8.17](spec.md#817-generators-p-766) play.
  - Other transitions play as cuts, and other generators play as gaps. Other effects do not play.
  - The Text and lower-third titles of Resolve come in as titles of vidcut. The titles of vidcut go out in the shape that Resolve uses ([D67](spec.md#12-decisions-on-gaps-and-contradictions)).
  - The Transform, Cropping, Dynamic Zoom, Composite and Retime and Scaling of Resolve come in as the picture of a clip. The picture of a clip goes out as these effects ([D75](spec.md#12-decisions-on-gaps-and-contradictions)).
  - Resolve Studio 21.1.1 imports `.otio` and `.otioz` files. It imports an `.otiod` folder by its `content.otio`.
  - It keeps tracks and their names, J/L cuts, clip gain, markers, stills, image sequences and the timecode of sources.
  - It also keeps clip effects and their keyframes, transitions, generators, speeds, and compound clips with their picture and sound linked.
- **Why:**
  - OTIO is an open standard that Resolve and other editors read and write. Thus a cut can go to a finishing tool and come back.
  - The EDL of Resolve keeps one video track, and no audio tracks or markers. Its FCPXML (1.10) keeps no markers.
  - vidcut carries what it does not model. Thus a cut that is finished in Resolve can come back for a recut and go back intact.

## X14. The timeline color space has no grading

- **Resolve:**
  - Color management converts each clip from its Input Color Space to the Timeline Color Space. Then it converts the result to the Output Color Space (pp. 226–232).
  - Its presets have SDR and HDR color spaces in Rec.709, P3, Rec.2020 and DaVinci Wide Gamut (pp. 230–231).
  - A new project has no color management (DaVinci YRGB). Then a dissolve is a straight line in the code values of the files.
  - With color management, every mix occurs before the output transform. The timeline color space changes the mix.
  - In a Rec.2100 ST2084 timeline, a dissolve is not a straight line in the PQ code values. Its working space is not known (measured in 21.1.1).
- **vidcut:**
  - The Timeline Color Space is Auto, SDR, HDR PQ or HDR HLG ([D50](spec.md#12-decisions-on-gaps-and-contradictions)).
  - Each clip goes into it from the tags of the clip. Where the tags are wrong, it uses the Input Color Space of the media ([D35](spec.md#12-decisions-on-gaps-and-contradictions)).
  - Every mix is a straight line in the code values of the timeline.
  - Each export converts the mixed frame to the colors that it selects ([D44](spec.md#12-decisions-on-gaps-and-contradictions)). The viewer uses the same math.
  - There is no grading.
- **Why:**
  - vidcut cuts. Grading belongs in other tools.
  - With Auto, mixed phone, camera and screen footage comes out correctly without color settings. The HDR video of a phone stays HDR.
  - The working space of Resolve's color management is not known. A straight line in the code values is what Resolve does without color management.

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

## X17. Image sequences are asked for, not guessed

- **Resolve:**
  - The Frame Display Mode decides whether numbered images are one sequence or stills.
  - Auto decides by file type (p. 366).
- **vidcut:**
  - Each image is a still, unless `media add --sequence` asks for the numbered run around it.
  - `media add-folder --sequence` asks for the runs in a folder ([D39](spec.md#12-decisions-on-gaps-and-contradictions)).
  - When the user imports numbered images in the GUI, picked or in a folder, vidcut asks which they are.
  - For frame formats (PNG, TIFF, EXR, DPX, TGA, BMP), the question has one clip for each run already selected.
- **Why:**
  - The file type does not tell the frames of a render from numbered photos.
  - A wrong guess turns a slideshow into a blink, or a render into thousands of stills.
  - A flag keeps scripts predictable.

## X19. HDR metadata comes from the sources

- **Resolve:**
  - An HDR export carries the metadata of the grading setup: the Mastering Display and the HDR10 metadata (p. 4117).
  - It also carries Dolby Vision and HDR10+ from the grade (pp. 151, 259–260).
  - Resolve Studio measures MaxCLL and MaxFALL (p. 282).
- **vidcut:**
  - There is no grade. Thus an export carries what its sources tell, or measures it from their frames ([D46](spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - Phone, camera and graded HDR footage looks on TVs as its sources do.
  - This needs no setup by a colorist.

## X21. Ctrl+wheel zooms the timeline

- **Resolve:**
  - Option+wheel zooms the timeline, and Command+wheel moves through it (Edit page, p. 853).
  - The lower timeline of the Cut page also zooms with Option+wheel.
- **vidcut:**
  - Alt+wheel (Option on a Mac) zooms the lower timeline, as in Resolve.
  - Ctrl+wheel (Cmd on a Mac) zooms it too, and does not move through it.
- **Why:**
  - Ctrl+wheel and Cmd+wheel zoom in browsers and most programs.
  - egui gives them and a pinch as one zoom.

## X24. Cross fades follow their curves exactly

- **Resolve:**
  - Resolve Studio 21.1.1 renders the curve of a Cross Fade as straight lines between points on the curve.
  - The points are the first sample, every 1000th sample from there and from the edit, and the last sample (at 48 kHz).
  - Early in a +3 dB fade, the gain is up to 0.06 off.
- **vidcut:**
  - The gain of each sample is the value of the curve ([D50](spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - The curve gives the transition its name. The curve needs no points from an editor.
  - The two differ by more than 1% only in the 20 ms where a curve leaves silence or reaches it.

## X25. Import is in the top bar

- **Resolve:**
  - The Import Media and Import Media Folder buttons are in the Media Pool (p. 654).
- **vidcut:**
  - Import and Import Folder are in the top bar.
- **Why:**
  - The top bar shows on every tab, also while Titles or the Inspector replaces the pool.

## X26. One folder for proxies and the render cache is a preference

- **Resolve:**
  - Proxies go to the "Proxy generation location" of the project (Project Settings, Working Folders, p. 145).
  - They go in subfolders by the folders of their sources (p. 208).
  - Preferences can put them in a "Proxy" subfolder beside each source instead, or ask for a folder each time (p. 98).
  - Render cache files go to the "Cache files location" of the project (p. 145).
- **vidcut:**
  - One preference, `playback_files.folder`, sets the folder of playback files. Proxies and the render cache go in it.
  - The default is the project's folder. A relative folder is in the project's folder. All projects share an absolute folder.
  - A second preference, `playback_files.render_cache`, sets a folder for the render cache only.
  - Projects that use the same file find and play one proxy. Each computer finds the proxy in its own folder.
- **Why:**
  - The location of proxies depends on the disks of the computer, not on the project. A folder saved in the project would name the disk of one computer.
  - One setting serves the two reasons to move proxies and the render cache. The first is a local disk for projects on a network drive. The second is one proxy for intros and outros that several projects use.
  - The render cache folder of its own keeps the two locations of Resolve. For example, the render cache can go on a faster disk.

## X27. A dissolve short of media is shortened

- **Resolve:**
  - Sometimes the clips do not have media for the full second at the edit. Then the Cut page of Resolve Studio 21.1.1 adds no dissolve and shows no message.
  - With no media past the cut, Ctrl+T on one selected side adds the full second. The missing frames are black.
  - The manual says that a transition is as long as the media of the clips allow (p. 1159).
  - On the Edit page, Ctrl+T asks whether to trim the clips or skip the edit (p. 1160).
- **vidcut:**
  - vidcut shortens the dissolve to the media that each side has, as the manual says ([D50](spec.md#12-decisions-on-gaps-and-contradictions)).
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

## X30. Slowed sound keeps its pitch on every system

- **Resolve:**
  - The manual says that K held with L plays forward slowly with pitch-corrected sound on macOS (p. 871). Thus on other systems, the slowed sound is lower.
  - K held with J plays backward with slowed sound.
  - Resolve Studio 21.1.1 on Windows plays the sound slowed. The pitch of that sound is not checked.
- **vidcut:**
  - Sound at any speed other than 1× keeps its pitch on every system ([D54](spec.md#12-decisions-on-gaps-and-contradictions)).
  - This includes slow play and playback backward.
- **Why:**
  - The behavior is the same on all systems.
  - Slowed speech at its own pitch is easy to follow when the user listens for a cut.

## X31. A file is in the pool once

- **Resolve:**
  - When the user imports a file into another bin, Resolve adds a second item for it, with its own media id.
  - When the user imports the file into the bin that holds it, Resolve adds nothing.
  - These results are from Resolve Studio 21.1.1.
- **vidcut:**
  - A file, or an image sequence, is one pool item.
  - When the user imports it again, it keeps its id.
  - Import Media Folder, or an import into a bin, moves it into that bin ([D53](spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - Scripts, the command log and OTIO files name media by id.
  - Two items for one file would split its marks, color and proxy.
  - An agent that imports a folder again would double the pool.

## X35. The History window docks

- **Resolve:**
  - The History window floats over the page (21.1.1).
- **vidcut:**
  - The history docks at the right side of the window ([spec §8.10](spec.md#810-undo-pp-651652)).
- **Why:**
  - A window over the picture hides what the history is about.

## X39. A GIF of one frame is a still, whatever its delay

- **Resolve:**
  - A GIF of one frame comes in as a still only when its frame has no delay.
  - With a delay, it is a video clip as long as the delay. For example, 0.1 s came in as 3 frames at 25 fps.
  - A WebP of one frame is a still, whatever the duration of its frame.
  - These results are from Resolve Studio 21.1.1.
- **vidcut:**
  - A GIF or WebP of one frame is a still ([D38](spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - The delay is how long a viewer shows the picture. It is not the length of a clip.
  - A clip of a few frames of one picture is a blink that the user cannot trim longer.
  - Thus GIF and WebP behave the same.

## X40. Subtitle files are read as they are written

- **Resolve:**
  - These results are from Resolve Studio 21.1.1. The TTML results are from Resolve Studio 21.2.
  - A Windows-1252 file shows U+FFFD for each accented letter.
  - Resolve leaves out a WebVTT cue that has no hours in its times.
  - From a TTML file, it keeps only the cues of the first region. With no named region, it imports nothing.
  - An italic span in a TTML cue makes the whole cue italic.
  - An italic span after a `<br/>` in a TTML cue does not come in italic.
  - A cue that starts before the previous cue ends starts at that end instead. When nothing of such a cue remains, Resolve leaves it out.
  - `{\an8}` shows as text.
  - Resolve ignores WebVTT `line`, `align` and color classes.
- **vidcut:**
  - It reads UTF-8, UTF-16 and Windows-1252.
  - It reads WebVTT times without hours, and every TTML cue, span and region.
  - It keeps cues that overlap.
  - `{\an8}`, WebVTT `line` and the place of a TTML region put a cue at the top, middle or bottom.
  - A WebVTT `position`, a `line` in percent, or a TTML region with an origin also gives the cue a position and an anchor.
    - A TTML region whose middle is the usual point of its place gives the place, but no position and no anchor.
    - A TTML cue without a region comes in at the bottom middle. Its lines still follow the alignment of the file.
  - WebVTT `align` gives the alignment of the lines. Without a `position`, it also puts the block at the left or right edge, as WebVTT does.
  - In TTML, `ebutts:multiRowAlign` gives the alignment of the lines. Without it, `tts:textAlign` does, and its usual value is start.
  - Italic, bold, underline and colors come in from all three formats, on the words that have them.
- **Why:**
  - An imported subtitle file must not lose text without a message.
  - A cue from a file shows where the file puts it.
  - A TTML cue without a region has no place in the file. Thus it takes the usual place of a subtitle.

## X41. Subtitle files placed by their timecode, or from the timeline's start

- **Resolve:**
  - A subtitle file is first a Media Pool clip.
  - Insert Selected Subtitles to Timeline Using Timecode places its cues by timecode. From a file timed from zero, it inserts nothing.
  - The user can drag the file onto a subtitle track. Then its first cue goes where the user drops it.
  - A drop on the gray area above the video tracks makes a new subtitle track. There too, the first cue goes where the user drops it (p. 1230).
  - These are measurements in Resolve Studio 21.2, on a timeline with one subtitle track:
    - A drop of a subtitle item on video track 1 makes no new track. Video track 1 does not change.
    - The cues go on the subtitle track from the drop point. They overwrite the cues there.
- **vidcut:**
  - Import Media, Import Media Folder and File > Import Subtitle bring in SRT, WebVTT and TTML files. vidcut does not read `.srtx`.
  - A subtitle item in the pool shows the text of its first cue, and how many cues it holds.
  - Insert Selected Subtitles to Timeline Using Timecode (`subtitles.import` with `media`) puts the cues of each file on a new subtitle track. The track has the name of the file.
  - vidcut reads a file as timecode when all its cues start at or after the start timecode of the timeline.
  - A second rule is for a timeline that starts at zero and is shorter than an hour. There, vidcut reads a file as timecode when all its cues start after 01:00:00:00.
  - Both rules use the time base of the file: real time or timecode labels ([D58](spec.md#12-decisions-on-gaps-and-contradictions)).
  - When neither rule applies, times count from the start of the timeline. On a timeline with a start timecode, the GUI tells this.
  - When the user drops a subtitle item on the timeline but not on a subtitle track, it makes a new subtitle track.
- **Why:**
  - A file timed from zero also goes on the timeline.
  - A subtitle file can only go on a subtitle track. Thus a drop at another place can only mean a new subtitle track.

## X43. A subtitle's text goes in as one change when the user leaves it

- **Resolve:**
  - The user types the text of a selected subtitle in the Captions panel of the Inspector.
  - The text shows on the clip while the user types (p. 1233).
  - A list at the bottom of the Captions panel shows every subtitle of the track (p. 1231).
  - Prev and Next move through the list (p. 1233).
  - The manual does not tell where the playhead goes.
  - These are measurements in Resolve Studio 21.2:
    - Next, and a click on a row of the list, select the cue and put the playhead at its In.
    - A click on a subtitle on the track does not move the playhead.
    - The typed text shows in the viewer at once. After a click at another place, one Ctrl+Z removes all of the typed text.
    - Insert Selected Subtitles to Timeline Using Timecode cuts the head of a later cue that overlaps. Cues at 11–13 s and 12–14 s become 11–13 s and 13–14 s.
  - In Resolve Studio 21.1.1, the panel is as the screenshots on pp. 1231 and 1233 show it:
    - Above the text box, a line has an In field and an Out field. At its right, a dim "11 Characters" counts the characters of the text.
    - Add New is at the left of the line above the list. Prev and Next are at its right.
    - The list has the columns "#" (the number of the row), "Time In/Out" (the In above the Out), "Caption" and "CPS".
  - These facts come from Resolve Studio 21.1.1:
    - Add New adds a subtitle at the playhead on the track of the list, as Add Subtitle in the right-click menu of the track does.
    - The new range overwrites the other subtitles of the track.
    - A subtitle that the new range fully covers is deleted.
  - These facts come from Resolve Studio 21.1.2:
    - An In inside the subtitle before, or an Out inside the subtitle after, changes nothing. Both subtitles stay.
    - The field then keeps the typed text until the subtitle is selected again. The list shows the old time.
    - A track has no overlaps. Thus a field cannot put a subtitle inside a longer subtitle.
- **vidcut:**
  - The Subtitle section of the Inspector tab has the In, the Out, the text and the position of the cue.
    - A double-click on a cue, Enter with one cue selected, Edit Subtitle… and Add Subtitle open it in the Inspector tab.
    - While the Inspector tab shows, a click on a cue also shows its section.
    - The position shows in the viewer when the user selects it.
  - The text goes in as one change when the user leaves the text box. The other rows go in at once. Ctrl+Z undoes a change.
  - A line under the heading has the In and Out fields, the duration and "N Characters", in that order.
    - Digits fill a timecode from the right, as in the timecode field of the viewer. A `+` or `-` before the value moves the edge by that much.
    - An In inside the cue before, or an Out inside the cue after, changes nothing. The field shows the value of the project again (D79).
    - Else the new range overwrites only the part that it newly covers. A cue of the track that lies fully inside that part is removed.
    - A cue that already overlaps this cue and reaches into the part loses its frames in the part.
    - A cue that covers all of the part and more stays as it is (D79).
  - `subtitles.set` alone does not overwrite. Scripts and imports keep the cues that overlap (D56).
  - The row of the open cue is highlighted. While the user types, it shows the typed text.
  - Prev, Next and a click on a row first put in the changes of the section. Then they select the cue and move the playhead to its start.
- **Why:**
  - One change for each edit of the text keeps typing out of the undo history. With the playhead at the start of the cue, the viewer shows the cue.
  - Imports and scripts can need cues that overlap (D56). Thus `subtitles.set` alone keeps them.

## X44. Add Subtitle uses the shown subtitle track

- **Resolve:**
  - The header of each subtitle track has a destination control (ST1, ST2, …).
  - This control selects the track that Add Subtitle puts new subtitles on (p. 1232).
  - Add Subtitle is in the right-click menu of a subtitle track (p. 1233).
- **vidcut:**
  - Add Subtitle in Timeline Actions puts the subtitle on the shown subtitle track.
  - Add Subtitle in the right-click menu of a subtitle track puts the subtitle on that track.
  - The header has no destination control.
- **Why:**
  - The track that the viewer shows is the track that the user works on. Thus one control does both.

## X52. Titles come from a Titles tab and are edited in the Inspector

- **Resolve:**
  - The user drags titles from the Titles browser onto the timeline (pp. 762–763, 1176).
  - The user edits the text of a title with a double-click in the Timeline Viewer. There, onscreen controls also move, scale and rotate the title (pp. 763–764, 1176).
  - The text is edited in place, inside a plain outline (measured in Resolve Studio 21.2).
  - The Video Inspector sets its fonts, colors, drop shadows, backgrounds and transform (p. 764).
- **vidcut:**
  - A double-click on a title in the Titles tab adds it at the playhead and opens the Title section of the Inspector tab.
  - A double-click on a title clip, or Edit Title… in its right-click menu, also opens the section.
  - While the Inspector tab shows, a click on a title clip also shows its section.
  - The section has the text, the kind of the title, the alignment and a Look list ([D71](spec.md#12-decisions-on-gaps-and-contradictions)).
  - It shows the place of the title, or of each line of a lower third, as percents across and down (X54).
  - A change goes in as one `titles.set` when the user leaves the field. A button puts it in at once. Ctrl+Z undoes it.
  - A value at its usual number shows an empty box with that number as a hint. A number in the other box of a pair keeps this usual number.
  - When the pointer is near a title, the viewer shows its outline, its corners and the handle above it. They work without a click.
  - Over them, the pointer becomes a move, resize or hand cursor. The corners zoom the title, and the pointer shows the zoom.
  - A scroll moves only across.
  - The title shows during the drag. One `titles.set` follows on release.
  - A double-click on the title in the viewer opens a text field over the title. The title changes as the user types. The double-click also opens the section.
- **Why:**
  - One change for each title keeps typing out of the undo history.
  - Handles that show on hover, and a cursor that changes over them, show that the user can drag them.
  - The zoom by the pointer shows when the zoom stops at its limit, 400%.
  - The text field is there until vidcut edits the text in place ([todo](todo/titles.md#text-in-the-viewer)).

## X53. Titles are Text, the three lower thirds and Scroll, styled by markup

- **Resolve:**
  - The Titles category holds L, M and R Lower 3rd, Scroll, Text and Text+, and Fusion Titles (p. 1178).
  - The lower thirds, Scroll and Text use rich text. Text+ does not.
  - The two lines of a lower third each have their own rich text, and their own Position, Zoom and Rotation (p. 1178).
- **vidcut:**
  - vidcut does not have Text+ and Fusion titles.
  - Markup in the text of a title styles words: `<b>`, `<i>`, `<u>` and `<font color face size>`.
  - The B, I, U, Color, Size, Font and Plain buttons of the Title section put this markup around the selected words.
  - The Position, Zoom, Rotation and Opacity of a title take keyframes ([todo](todo/keyframes.md#titles)). Its shadow and background do not.
  - A title clip has no Transform, Cropping, Speed or blend modes.
- **Why:**
  - Text+ and Fusion titles are Fusion compositions, and vidcut does not have Fusion (spec §1). Blend modes are out (spec §1).
  - The place, zoom, rotation and opacity of a title do what Transform and the opacity of Composite would do. The speed of a scroll comes from the length of its clip.
  - Markup in the text is what a script writes and reads.

## X54. A title's place shows as percents from the top left

- **Resolve:**
  - The user moves the text of a title with a drag in the viewer. It snaps to the center and the thirds of the frame (pp. 763–764, 1176).
  - The Position in the Inspector also moves it (p. 764).
  - A lower third is at a corner or at the middle of the bottom of title safe (p. 1178).
  - A position goes from 4 picture sizes before its bottom left to 4 picture sizes after it, on each axis (measured in Resolve Studio 21.1.2).
  - The Position of a Text title in the Inspector is in frame pixels from the bottom left, with Y up (measured in Resolve Studio 21.2).
    - In a 3840 × 2160 timeline, the middle of the picture is X 1920, Y 1080.
- **vidcut:**
  - The Title section of the Inspector shows the place of a title from the top left of the picture.
  - It shows the place in percents of the width and the height of the picture.
- **Why:**
  - Percents mean the same at any picture size.
  - The y counts down from the top. Thus it is 1 minus the y of Resolve's position, and the limits are the same.

## X55. A scroll rises over the whole clip, its lines spaced as other titles'

- **Resolve:**
  - Scroll moves its text from the bottom of the picture to the top. The length of the clip sets the speed (p. 1178).
  - These are measurements in Resolve Studio 21.1.1:
    - The middle of the first line is on the bottom edge one frame before the clip starts.
    - The text rises at a steady speed.
    - How far it goes past the height of the frame changes with the text size and the picture size. It is 1.9 to 2.9 times the text size.
    - Thus the text leaves the top before the clip ends.
    - Its default lines align left, at 1% of the width of the picture.
    - Its text box stays at 51% of the width when the alignment changes. Thus its middle alignment is 1% right of the middle, and its right alignment goes 1% past the right edge and cuts the last letters.
- **vidcut:**
  - A `scroll` title rises at a steady speed.
  - It starts just below the picture at the first frame of the clip. It ends just above the picture at the last frame.
  - Its middle alignment is at the middle of the picture. Its right alignment ends at 99% of the width.
- **Why:**
  - Credits need a scroll that uses the full clip, and that starts and ends clear of the picture.
  - The distance in Resolve cannot be found from the size of the text and of the picture.
  - A right alignment that cuts letters is a fault in Resolve.

## X56. Create Subtitles from Audio reads each clip's file

- **Resolve:**
  - Create Subtitles from Audio analyzes the speech in the timeline.
  - It "commonly fails" where dialog overlaps on several tracks. The manual says to mute the tracks that are not wanted (p. 1235).
  - A timeline transcription, in the Transcription window, transcribes the media of each clip through its In and Out instead (p. 1001).
- **vidcut:**
  - By default (`hear: media`), `subtitles.transcribe` reads the words of the file of each heard clip.
  - It uses the words kept with the file when the same model made them in the requested language. Otherwise it transcribes the whole file and keeps its words.
  - It places the words through the range and the speed of each clip, J and L cuts too.
  - Two tracks can have the same word at the same time. Then it keeps the word of the first track (V1, V2, …, then A1, A2, …).
  - `hear: mix` hears the mix of the tracks over the range and keeps nothing. It is the default over In–Out or any other part of the timeline.
  - vidcut hears the sound of a compound clip only in the mix.
  - When a heard file has a transcript, the first row of the dialog, Words, offers From Transcript or Choose Model.
  - From Transcript is selected, unless the user selected Choose Model before.
  - When every heard file has a transcript, From Transcript names the model and the language that made them. It needs no model.
  - Otherwise, From Transcript names the files that it transcribes first, with the selected model.
  - Choose Model uses again what the selected model made before. With it, or with no transcript, Words From under Advanced selects the file or the mix.
  - The dialog tells how many heard files it transcribes first, how long they are, and how many transcripts of another model it replaces.
- **Why:**
  - After one transcription, the words of a file serve subtitles, the Transcription window and cutting by words, in every project that uses the file.
  - Subtitles from an existing transcript take a moment and cost nothing. A file heard alone is clearer than the mix.
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
  - Synchronize Word Timings is in the right-click menu of the header of a subtitle track on the Edit page (Resolve Studio 21.2).
  - Measured in Resolve Studio 21.1.2:
    - The word times of a cue count from its start. A ripple delete, a razor cut and a trim do not change them.
    - The highlighted word is then not the word that is spoken, until the word timings are updated.
- **vidcut:**
  - The animation is part of the style of a track (D69). The user sets it in the Subtitle Track section of the Inspector or with `track.set`.
  - The looks of Create Subtitles from Audio ([spec §8.12](spec.md#812-subtitles-pp-645646-702-12271248)) put the speech on a new track that is already animated.
  - Word times follow edits and new text automatically (D70).
- **Why:**
  - It is one step from the speech to animated subtitles.
  - Fusion is not necessary. A look is a style, and the user can change it in the Inspector.
  - Word times stay correct after an edit or a correction, and there is no button to remember.

## X60. Clip Info is a window

- **Resolve:**
  - The lower-right corner of a thumbnail opens an overlay of its information (p. 353).
  - The File tab of the Inspector shows the clip details and edits the logging fields (p. 412).
- **vidcut:**
  - The info button of the metadata strip or of a thumbnail corner opens Clip Info, a window in the place of an overlay ([spec §3](spec.md#3-media-pool-ch-34-pp-653675)).
- **Why:**
  - One window with all the information, read fresh from the file, tells the format without a separate tool.

## X61. Speakers are numbered per file or per run, and joined by hand

- **Resolve:**
  - Speaker Detection (Studio) remembers each voice across the clips of a project. Thus it finds a speaker from one clip again in the next clip.
  - Reset Speaker Data forgets the speakers (pp. 997–1001).
- **vidcut:**
  - On this computer, speakers come numbered for each file.
  - With a paid request, speakers are numbered across all files of the request.
  - Same Person As, in the right-click menu of a name in the Transcription window, joins the speaker of this file with another person.
  - vidcut cannot join speakers by their voices yet, in a project or across projects. This waits for transcribe.cpp ([to do](todo/voice-signatures.md)).
- **Why:**
  - No model that vidcut runs gives the signature of a voice yet.
  - To join the numbers by hand once in a project is quick.
  - An automatic join that is wrong would be worse than no join.

## X66. Files and folders dropped on the window go into the open bin

- **Resolve:**
  - Clips dragged from the file system go into the Media Pool. A drop on a bin in the Bin list puts them in that bin (p. 364).
  - For the Cut, Edit, Fusion and Fairlight pages, the manual says macOS only (p. 372).
  - Folders dragged from the file system into the Bin list come in as bins, nested as they were (p. 365).
  - The manual does not tell what a folder dropped on the Media Pool of the Cut page does.
- **vidcut:**
  - With a project open, files and folders dropped anywhere on the window go into the open bin.
  - The files come in as Import Media brings them in.
  - Each folder comes in as Import Media Folder brings it in: a bin, with a bin for each folder in it.
  - While they are over the window, the Media Pool shows Drop to Import.
  - With no project open, a drop does nothing, because Import is not available then.
- **Why:**
  - The window does not get the place where files are dropped. Thus a drop cannot aim at a bin or the timeline. The open bin is where Import puts media.
  - A folder keeps its folders as bins, as the Bin list of the manual does. The Cut page has a Bin dropdown, not a Bin list (p. 655).

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

## X68. Ducking times every track as a mono track

- **Resolve:**
  - The Ducker times a mono target track by its controls (pp. 778–779; [D76](spec.md#12-decisions-on-gaps-and-contradictions)).
  - On a stereo target track, Rise Time, Hold and Recovery Time act at half their values.
  - The fall starts at the same time as on a mono track.
  - The two channels get different gains while the gain changes.
  - These facts come from renders of Resolve Studio 21.1.1. The cause of these differences is not known.
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
  - A fixed threshold, measured between −48 and −36 dBFS peak, makes a source active ([D76](spec.md#12-decisions-on-gaps-and-contradictions)).
  - With a Rise Time of 10 ms, the fall is a one-pole curve on the linear gain, not a half cosine.
  - With a Rise Time of 200 ms, the fall is a half cosine of 202 ms.
  - Each fall starts at a whole multiple of 10 ms from the start of the timeline.
  - A fall starts Lookahead + Rise Time + 10 to 15 ms before the source.
  - The gain changes in steps of 16 samples.
  - A new Ducker on a timeline with two audio tracks has the other track as Source 1. With three tracks, Source 1 is empty.
  - A typed Recovery Time of 2000 ms becomes 1000 ms.
- **vidcut:**
  - Detect selects Speech, the default, or Sound level.
  - Speech: a source is active where Silero VAD hears speech in it.
  - Sound level: a source is active where its peak is at or above the Threshold.
  - The Threshold is −42 dBFS by default, the middle of the measured range. It is under Advanced.
  - The fall is a half cosine in dB over the Rise Time, for every Rise Time.
  - Each fall starts Lookahead + Rise Time before the source, with no 10 ms steps.
  - The gain changes on each sample.
  - A new ducking has every other track with sound as its sources.
  - `track.set` refuses a value outside its range.
- **Why:**
  - Room noise, breaths, bumps and music on a dialogue track are louder than a fixed threshold. A speech detector tells speech apart from them.
  - Sound level keeps the behavior of the Ducker, for sound effects and music as sources.
  - One fall shape for every Rise Time is easier to predict. A start on a grid of 10 ms is of no use to the user.
  - The usual mix ducks music under every other track. A value that changes without a word is a different setting than the one asked for.

## X70. The Inspector is a tab beside the Media Pool, in frame pixels

- **Resolve:**
  - The Inspector button at the top right of the UI toolbar opens the Inspector (pp. 734–744; [spec §8.15](spec.md#815-transform-and-crop-pp-766768)).
  - Its Video panel has Transform, Cropping and Dynamic Zoom. Each group has an on/off toggle and a Reset of all its values.
  - There is no Reset for one value.
  - Position X and Y are timeline pixels from the middle, with Y up.
  - A picture narrower than the frame moves less than its number. On a 4:3 picture in a 16:9 timeline, Position X 200 moves it 150 pixels.
  - Crop is in pixels of the fitted picture. Zoom is a factor, and Rotation is in degrees.
  - The Audio panel has Pan, Pitch, Equalizer, Voice Isolation, Dialogue Leveler and Music Remixer (pp. 743–744).
  - The manual names no Mute in the Audio panel (pp. 742–744).
  - Its Video panel also has Speed Change: Direction, Speed %, Frames Per Second, Duration, Ripple Sequence and Pitch Correction (pp. 736–737).
  - These are measurements on the Cut page of Resolve Studio 21.2:
    - The Speed Change rows are Direction, Ease, Change Speed, Frames per Second, Duration and Pitch Correction. There is no Ripple Sequence box.
    - Pitch Correction is on. A speed change ripples the clips after the clip.
  - With several clips selected, its header is Multiple Clips, and a value that differs between them shows a dash (measured in Resolve Studio 21.1.2).
- **vidcut:**
  - An Inspector tab is beside the Media Pool and Titles tabs. It opens in the same place.
  - It shows the Title section of a title and the Subtitle section of a subtitle. Other clips get the picture, Speed, Audio, Equalizer and Dialogue Leveler sections.
  - Of the Title, Subtitle and Subtitle Track sections, one shows at a time (X43, X52). Another selection puts its changes in first. A field that is not valid then loses its change.
  - With nothing selected that it can show, it shows one line.
  - With several clips selected, a section shows the first clip that it acts on. Its header gives the number of these clips.
  - The Transform, Crop, Equalizer and Dialogue Leveler sections have a switch at the left of their names. The Pan row has a switch too. Dynamic Zoom, Composite, Speed and the other Audio rows have none.
  - Dynamic Zoom Off takes the Dynamic Zoom off the clip.
  - The label of a value that is not at its default is in the accent color. Such a value has its own Reset button.
  - With several clips, the accent color and the Reset show when one of the clips has such a value. A value that is off has no accent color, and its Reset does nothing.
  - The link button of Zoom turns the link on again, and makes Zoom Y equal to Zoom X. A picture with two different zooms shows the link off.
  - **Dynamic Zoom:** Off or On, Start Zoom, Start X, Start Y, End Zoom, End X, End Y, the Ease, Show Boxes and Swap.
    - A value typed while Dynamic Zoom is off turns it on.
    - Show Boxes shows the start box and the end box in the viewer, as Alt+T does. On also shows them.
    - If the playhead is not on a selected clip, Show Boxes and On move it to the first frame of the first selected clip.
  - **Speed:** there is no Pitch Correction. Sound always keeps its pitch (D48).
    - Ripple Timeline starts as Ripple On. A change of it applies to the Inspector only.
  - **Audio:** Volume, Mute and Voice Isolation of the selected clips with sound.
    - Mute does what Mute and Unmute in the right-click menu of the clip do.
    - A drag on Volume sends one change on release. The sound does not change during the drag.
    - The Audio section has no Reset of the whole section.
    - Normalize in the header opens Normalize Audio Levels for the clips of the section ([spec §9](spec.md#9-audio-pp-642-649650-743744-771-773-778779)).
    - Voice Isolation and its Amount do what Voice Isolation in the right-click menu does, with the same download question ([spec §9](spec.md#9-audio-pp-642-649650-743744-771-773-778779)).
    - Pan sets the pan of the selected clips ([X153](#x153-every-clip-pans-as-on-a-stereo-track)).
    - The Compressor row has a switch and a button that opens the Compressor window ([X156](#x156-the-compressor-is-on-a-clip)).
    - The Noise Reduction row has a switch and a button that opens the Noise Reduction window ([X157](#x157-noise-reduction-is-our-own-spectral-gate)).
    - The Audio section has no Pitch or Music Remixer.
  - **Units:**
    - Position X and Y are frame pixels from the middle of the frame, with Y up. Thus Position X 200 moves every picture 200 frame pixels, whatever its shape.
    - Zoom is in percent, as the viewer shows it while the user zooms.
    - Anchor X and Y are pixels of the fitted picture from its middle, with Y up. At 100% zoom, these are frame pixels.
    - The zoom of a Dynamic Zoom box is in percent. The X and Y of a box are where the middle of the picture is at the start or at the end.
    - They are in pixels of the fitted picture from the middle of the clip, with Y up. A new zoom of a box keeps the middle of the picture in place.
- **Why:**
  - The switches of the other sections are not made yet ([gaps](todo/gaps.md)).
  - The Titles tab of the Cut page is in the place of the Media Pool. The Inspector goes beside it. Thus the viewer and the timeline keep their size.
  - A number in frame pixels does the same thing to every picture. The viewer shows the zoom in percent. Thus one value does not show in two forms in vidcut.
  - A Reset for each value puts back one value and keeps the others of its group.
  - Resolve's Inspector has only Ease and Swap for Dynamic Zoom. Numbers show each box exactly, and bring back a box that is out of the viewer.
  - Mute is beside Volume, as in the strip of the Mixer. Thus the user does not need the right-click menu to mute a clip.

## X73. The volume line takes the keyframe controls of the Edit page

- **Resolve:**
  - The timeline of the Cut page shows no volume overlay, and no keyframe marks on clips (Resolve Studio 21.1.1).
  - On the Cut page, the Inspector and the Keyframe Editor set volume keyframes (pp. 743, 1255).
  - On the Edit page, the volume overlay of a clip takes keyframes (pp. 1109, 1112–1113).
  - In Resolve Studio 21.1.1, a click on a keyframe of the overlay adds it to the selection.
- **vidcut:**
  - A new keyframe from an Alt-click on the volume line is selected.
  - A click on a keyframe selects only that keyframe. Shift adds it to the selection. Ctrl adds it or takes it out.
  - A click on another part of a clip takes the keyframes out of the selection.
  - A dragged keyframe does not pass its neighbors.
  - Before the first or after the last keyframe, a drag on the line moves that keyframe.
  - The right-click menu of a keyframe also has Delete Keyframes.
  - During a drag, the timeline shows the new curve and the volume beside the pointer.
  - A selected keyframe with an ease shows its handles ([X74](#x74-eases-are-bezier-handles-that-drag-on-the-volume-line)).
- **Why:**
  - vidcut's timeline has the volume line. The controls of the Edit page fit it, and the user does not need a second editor for the volume.
  - A click selects only that keyframe, as a click on a clip selects only that clip. Shift and Ctrl add to the selection, as they do for clips.

## X74. Eases are Bezier handles that drag on the volume line

- **Resolve:**
  - In Resolve Studio 21.1.1:
    - An ease is a Bezier handle on its keyframe.
    - With Ease Out on one keyframe and Ease In on the next one, the volume holds the first value. It jumps at the second keyframe.
  - The manual says that an ease starts or stops a change slowly (p. 1112).
  - The handles drag in the Curve Editor (p. 1261), not on the volume overlay.
- **vidcut:**
  - The curve between two keyframes is always the Bezier curve of the keyframes and their handles ([D81](spec.md#12-decisions-on-gaps-and-contradictions)).
  - Thus Ease Out followed by Ease In gives a smooth S-curve.
  - A selected keyframe with a handle shows the handle on the volume line. A drag on the handle changes its time and its value.
- **Why:**
  - The S-curve is what the manual describes, and what the handles give. A hold and a jump is a step in the sound.
  - A handle on the line changes the curve where the user sees it, without a second editor.

## X80. Safe area guides skip the Trim Editor and Dynamic Trim

- **Resolve:**
  - The Safe Area button and View > Safe Area set overlays on the viewer (pp. 682–683, 1177).
  - In Resolve Studio 21.2, the Trim Editor also shows the title-safe guide. It is one box over the whole two-up view, fitted to the viewer. It is not one box for each picture.
- **vidcut:**
  - The Trim Editor and Dynamic Trim show no guides. Those views are two pictures, or four.
  - A viewer screenshot is the picture without the guides.
- **Why:**
  - The guides belong on the one picture fitted to the timeline.
  - One box over two pictures marks neither picture. vidcut does not copy it.
  - A two-up view has no single frame for one crop.

## X83. Pitch and yaw are Inspector values

- **Resolve:**
  - The onscreen Transform controls set Zoom, Position, Rotation, Pitch, Yaw and Flip (p. 767).
  - The Inspector sets Pitch and Yaw too (p. 735).
- **vidcut:**
  - The Transform handles in the viewer do not set Pitch or Yaw.
- **Why:**
  - Pitch and yaw are numbers, as Rotation is.
  - The Inspector is where those numbers are set.
  - The viewer handles move, zoom, turn and crop the picture.

## X86. The Full Screen button is on the viewer

- **Resolve:**
  - The Full Screen control is at the right of the top bar (p. 636).
- **vidcut:**
  - The Full Screen button is on the viewer, beside the duration.
- **Why:**
  - Full screen and the enhanced viewer cover the top bar.
  - The button stays on the viewer, so it can be clicked while they are on.

## X90. The curve of a value shows under its track

- **Resolve:**
  - The Curve Editor is a separate view (p. 1255).
- **vidcut:**
  - The curve of a value shows under its track in the Keyframes tab.
- **Why:**
  - The curve stays in line with the keyframes that it joins.

## X91. Super Scale uses the upscaler of the GPU

- **Resolve:**
  - Super Scale is in the Studio version only (p. 288).
  - It is a setting of a media clip, in Clip Attributes and in the Inspector of the Media page (pp. 288, 432–433).
  - It has 2x, 3x and 4x, each also as Enhanced. Sharpness and Noise Reduction are Low, Medium or High. Enhanced makes them variable (p. 433).
  - The project setting also has Auto (p. 149).
  - It makes a clip of a higher resolution, which often stops playback in real time (p. 288). The usual sizing then fits that clip.
- **vidcut:**
  - Super Scale is Off or On. On enlarges a picture as much as it needs, up to 4 times its source. There is no Enhanced mode.
  - Its Upscaler is Project Settings, the default, AI Slow, AI Fast or Classic.
  - Project Settings has the upscaler of the project: Auto, the default, AI Slow, AI Fast or Classic. Auto is AI on macOS, and AI Fast elsewhere.
  - The lists name the upscaler that Auto and Project Settings give on this computer, as in Auto (AI Fast).
  - On macOS, the upscalers are AI and Classic. AI Fast and AI Slow are off there: exports and the viewer refuse them.
  - AI on macOS is the super-resolution scaler of macOS 26 and later.
    - It takes SDR and HDR pictures up to 1920×1080, the limit of macOS for video.
    - macOS downloads its models only when the user agrees. macOS does not give their size, so the question says that the size is unknown.
  - AI Slow is the one-step diffusion model FlashVSR+ Turbo. It is slow, and it adds more detail. It has no setting.
  - AI Fast is the video model NanoVSR 5.4M. It has no Noise Reduction and no other setting. It takes SDR and HDR pictures of any size.
  - The weights of AI Slow (7.0 GB) and AI Fast (25 MB) download at their first use. vidcut does not ship them.
  - vidcut refuses a picture when the picture needs enlarging and the upscaler cannot take it.
    - In an HDR timeline, AI Slow refuses HDR media ([X98](#x98-in-an-hdr-timeline-ai-slow-works-before-the-conversion)).
    - The viewer and an export use one rule and one message. The message names the file and the reason.
    - An export refuses before it starts. The viewer shows the same message over its picture.
    - An export also refuses an AI upscaler that it needs without its weights, or without a GPU that can run the network.
  - The Inspector of the Cut page has a Super Scale section for the media of the selected clips.
  - The viewer shows Super Scale only with View > Show Super Scale, which is off by default.
  - With an AI upscaler, the viewer runs the upscaler of the export on each picture that the export enlarges. With AI Slow, a picture can take seconds.
- **Why:**
  - A video model adds detail that a classic upscaler cannot. Some users wait a long time for the best pictures, and others want good pictures fast.
  - vidcut makes a picture only as large as the output shows it. A factor would only limit placed pictures. Thus On is sufficient.
  - The owner chose the scaler of macOS as the default on macOS. The owner also decided that AI Fast and AI Slow stay off there. Thus an export has different pixels on macOS and on other systems, and the owner accepts this.
  - The owner approved the defaults of Super Scale as better than those of Resolve.

## X92. Deleting words on the timeline cuts at quiet points

- **Resolve:**
  - Delete of words in Timeline Transcription mode cuts exactly at the word times of the transcript.
  - The manual does not say this (pp. 1006–1008). Resolve 21.1.1 was tested.
- **vidcut:**
  - Delete, Remove Silent Portions and Delete Filler Words on the timeline move each edge to the quietest frame nearby (D63).
- **Why:**
  - The word times of a speech model are approximate.
  - A cut at those times can take the start of the next word, or leave part of a breath or a click.
  - A cut at a quiet point sounds cleaner.

## X93. Retime Process has AI models in the place of Optical Flow

- **Resolve:**
  - Retime Process is in the Retime and Scaling group of the Inspector (p. 739).
  - Its values are Project Settings, Nearest, Frame Blend and Optical Flow. The default is Project Settings.
  - The project's Retime Process is in Master Settings, Frame Interpolation (p. 145). In a new project of Resolve Studio 21.1, it is Nearest.
  - Optical Flow has a Motion Estimation mode (p. 740): Standard Faster or Better, Enhanced Faster or Better, and Speed Warp Faster or Better.
  - Speed Warp uses the DaVinci Neural Engine. AI Speed Warp Metal is a version of it for macOS only.
- **vidcut:**
  - The Speed section of the Inspector tab and `clip.set` set Retime Process.
  - The Change Clip Speed window (R) also sets it, for the selected clips with a picture.
  - Its values are Project Settings, the default, Nearest, Frame Blend, AI Fast and AI Slow.
  - The Retime Process of the project is Auto, the default, Nearest, Frame Blend, AI Fast or AI Slow.
    - Auto gives slow motion the AI of the computer that plays or exports: AI on macOS, AI Fast elsewhere.
    - Auto gives Nearest to the rest: clips at 100% or faster, and frozen and backward clips.
  - The lists name the process that Auto and Project Settings give on this computer, as in Project Settings (AI Fast).
  - On macOS, the processes are Nearest, Frame Blend and AI. AI Fast and AI Slow are off on macOS.
    - AI is the frame rate conversion of macOS. macOS does not download a model for it.
    - An export on another system refuses AI, and an export on macOS refuses AI Fast and AI Slow.
  - AI Fast (RIFE 4.25) and AI Slow (EMA-VFI) are AI models. Each makes the frame from the motion between the two source frames.
    - The models make a frame at any point between the two source frames.
    - Their weights download at their first use: 25 MB and 264 MB. vidcut does not ship them.
    - Before a change that makes the timeline use a model that this computer does not have, the GUI asks to download it.
  - Between two source frames with a shot change, the frame is the nearer source frame.
- **Why:**
  - The AI of vidcut needs no paid version. Thus slow motion gets it by default.
  - Other speeds and frame rates gain less from made frames. Thus Auto keeps the speed of Nearest for them.
  - A morph from one shot into the next looks wrong. Thus a shot change keeps the nearer source frame, as Nearest does.
  - The owner approved the Auto default as better than the Nearest default of Resolve.

## X94. Speed keyframes and the Mark menu's keyframe items

- **Resolve:**
  - Add Keyframe, Delete Keyframe and Delete All Keyframes act on the selected values of the Keyframes panel or the keyframe tray (p. 4343).
  - With the panel and the tray closed, they act on the speed keyframes of the selected clips.
  - Then Delete Keyframe and Delete All Keyframes do the same thing in Resolve Studio 21.1.2.
    - Each removes one speed keyframe: the one at the playhead, or else the nearest one before it. No question opens.
  - With the tray open and no row selected, Add Keyframe also adds a speed keyframe.
  - In Resolve Studio 21.1.2, the three items are always on, also when they cannot act.
  - Delete All Keyframes with no keyframe selected asks "Delete All Keyframes?". The question names the value and the media file.
  - With no row and no keyframe selected, the question names a value that no row shows as selected.
  - In one check, the question named no value. After Delete, a Position X with no keyframes also went back to 0.
  - The keyframe tray has a Speed group with two rows, Retime Frame and Retime Speed. Each row has a key at each speed keyframe.
- **vidcut:**
  - The Keyframes tab and the keyframe tray have one Speed row, in a Speed group.
  - With the Keyframes tab or the tray open, the three items act on the selected rows and keyframes.
  - With the tray open, the Keyframes tab closed, and no row or keyframe selected, Add Keyframe adds a speed keyframe, as in Resolve.
  - With the Keyframes tab open and no row selected, Add Keyframe stays off.
  - With both closed, they act on the speed keyframes of the selected clips ([D48](spec.md#12-decisions-on-gaps-and-contradictions)).
    - Delete All Keyframes then asks first, for example "Delete 3 Speed keyframes of Clip 2?". Then it removes all the speed keyframes.
  - An item that cannot act is off. Its tooltip tells the next click, for example "Click a parameter name, such as Opacity".
  - When the three items are off for one reason, a line in the menu tells the next step, for example "Select a clip first".
  - With no row and no keyframe selected, Delete All Keyframes is off.
  - The question of Delete All Keyframes names the values, the number of keyframes and the clip.
  - Delete All Keyframes removes only the keyframes of the selected rows. A value with no keyframes keeps its value.
- **Why:**
  - Retime Frame and Retime Speed show the same speed keyframes, so one row is sufficient.
  - An item that does nothing gives the user no clue. The user sees which keyframes go, and how many, before they go.
  - A Delete All that removes one speed keyframe is a fault in Resolve. The reset of a value with no keyframes is a fault too. vidcut does not copy them.

## X95. A speed change keeps the last source frame

- **Resolve:**
  - The last key of a clip's speed is one frame of the retimed media before the media's end. Its source time is the media's end less the speed of the last section.
  - Thus a section slower than 100% at the end of the media does not show the last source frame.
  - Example in Resolve Studio 21.1.2: a 900-frame clip, a speed keyframe at frame 30, and 50% after it. The clip shows source frames 0 to 898.
  - The manual does not tell this.
- **vidcut:**
  - The last section shows each source frame to the end of the media, the last frame too ([D48](spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - A speed change does not remove pictures from a clip. The lost frame is a fault in Resolve. vidcut does not copy it.

## X96. Reverse keeps the speeds of the other sections

- **Resolve:**
  - The Direction buttons of the Inspector act on the section under the playhead.
  - Example in Resolve Studio 21.1.2: speed keyframes at frames 30 and 60. The middle section plays at 200%, and the last at 100%. The clip has 884 frames.
    - After Reverse, frames 31 to 45 show source frames 28, 26 and so on to 0. The section plays backward to the media's first frame.
    - Frames 46 to 494 show source frames 2, 4 and so on to 898. The last section changes to 200%, and the clip has 495 frames.
  - The manual does not tell this.
- **vidcut:**
  - The first section has no speed keyframe at its start. Reverse turns it at the speed keyframe where it ends.
  - The sections after the turned section go on from where it stops, at their own speeds. The speed keyframes and the clip's length stay (`speed.reverse`, [D48](spec.md#12-decisions-on-gaps-and-contradictions)).
  - When the turn needs frames that the media does not have, Reverse refuses.
- **Why:**
  - The user changes the direction of one section. The other sections keep their speeds.
  - In Resolve, the next section changes its speed, and the clip gets shorter. vidcut does not copy this.

## X97. Ease In and Ease Out ease their own side of a section

- **Resolve:**
  - The Ease list of the Inspector acts on the section under the playhead. It has Linear, Ease In, Ease Out and Ease In and Out.
  - Example in Resolve Studio 21.1.2: speed keyframes at frames 30 and 60, and 25% between them.
    - Ease In, Ease Out and Ease In and Out each ease only the speed keyframe at frame 30. They give the same export and the same render.
    - The speed keyframe at frame 60 stays linear. There, the speed changes at once.
  - The manual does not tell this. Its Speed Change controls of the Cut page have no Ease list.
- **vidcut:**
  - Ease In eases the speed keyframe where the section starts. Ease Out eases the one where it ends. Ease In and Out eases both, and Linear neither.
  - The list shows the eases of the two speed keyframes of the section (`speed.ease`, [D48](spec.md#12-decisions-on-gaps-and-contradictions)).
  - The first section has no speed keyframe at its start, and the last none at its end. Their lists offer only the eases that apply.
  - The Speed row of the Keyframes tab and the tray offers Linear and Ease In and Out for one speed keyframe.
- **Why:**
  - Three choices that give the same result are a fault in Resolve. vidcut does not copy it.
  - Ease In and Ease Out then do what their names tell, as the Dynamic Zoom ease does. Ease In is slow at the start, Ease Out at the end.
  - A handle on one side of a speed keyframe alone gives a straight line. Thus one speed keyframe takes an ease on both sides or on none.

## X98. In an HDR timeline, AI Slow works before the conversion

- **Resolve:** the input goes into the timeline color space before Super Scale and before Speed Effects (§13, p. 3407).
- **vidcut:**
  - In an HDR timeline, AI Slow takes SDR media as SDR. The draw then converts its result into the timeline color space.
  - AI Slow refuses HDR media in an HDR timeline ([X91](#x91-super-scale-uses-the-upscaler-of-the-gpu)).
- **Why:**
  - In the test of `hdr_models` ([tools](tools.md)), AI Slow in PQ and HLG came far from the real frames, also with its color fix.
  - SDR goes into an HDR timeline with no clipping. Thus the conversion after the stage keeps all of the picture.

## X99. Smooth Cut has a Process row in the place of Speed Warp

- **Resolve:**
  - A Smooth Cut has a Speed Warp box in the Transition Inspector (p. 759). It is on by default.
  - With the box on, Speed Warp, an AI model, makes the frames. With the box off, the older optical flow makes them.
  - When the optical flow does not find the motion, the frames are a cross dissolve ([gotchas](gotchas.md#resolves-otio-files)).
- **vidcut:**
  - A Smooth Cut has a Process row in the place of the box. Its values are the processes of Retime Process ([X93](#x93-retime-process-has-ai-models-in-the-place-of-optical-flow)).
  - The values are AI Fast, AI Slow and Frame Blend. AI Fast is the default.
  - On macOS, the values are AI and Frame Blend. AI is the default.
  - A request sets it as `process`. `ai` is the AI of the computer that plays or exports.
  - An OTIO file holds AI Fast, AI and AI Slow as Speed Warp on, and Frame Blend as Speed Warp off.
- **Why:**
  - vidcut has no optical flow. Its AI models make the frames in the place of Speed Warp and of the optical flow.
  - AI Slow makes frames nearer to the real ones, in more time. The row lets the user choose it.
  - Frame Blend makes the frames with no model and no download.

## X100. A new Smooth Cut is 0.2 seconds long

- **Resolve:**
  - The Smooth Cut button puts a one-second Smooth Cut, the default length of all transitions (p. 759).
  - It also puts a one-second Cross Fade 0 dB on the sound.
  - The manual tells that 2- to 6-frame Smooth Cuts often work best (pp. 759, 1166).
- **vidcut:**
  - The Smooth Cut button and the Smooth Cut card put a 0.2-second Smooth Cut, to the nearest frame. At 30 fps, it is 6 frames.
  - The Cross Fade 0 dB on the sound has the same length.
  - `edit.dissolve` with a smooth cut and no `length` gives 0.2 seconds too.
- **Why:**
  - A one-second morph shows a long blend of two positions of the speaker.
  - A one-second sound fade over a cut in speech plays the voice twice, with an echo.
  - In a test on a cut pause of an interview, 6 frames at 30 fps looked and sounded good.

## X101. Black and Replicate borders follow their plain definitions

- **Resolve:**
  - Border Type sets what a Gaussian Blur takes from beyond the edges of the frame (p. 3536).
  - Measured in Resolve Studio 21.1.2, at Strength 1 on a 1920-pixel-wide timeline, next to a white edge:
    - Black is darker in the last 8 pixels than a blur of one pass with black beyond the edge. The value at the edge is 0.06, not 0.27.
    - Replicate takes about 60% of the edge color. The value at the edge is 0.64, not 0.75.
  - No blur of one or more passes that was tried fits these values.
  - Reflect and Wrap-Around match a blur of one pass.
- **vidcut:**
  - Black takes black and clear pixels from beyond the edges.
  - Replicate takes the pixels of the edge, repeated.
- **Why:** the measurements do not fit a known blur. The plain definitions give the result that the names tell.

## X102. A portrait timeline blurs by the width rule

- **Resolve:**
  - The sigma of a Gaussian Blur is 0.05625 × Strength² × the timeline width in pixels ([spec §8.16](spec.md#816-effects-pp-765-3526-3536)).
  - Timelines of 1920×1080, 1920×1440 and 3840×2160 fit this rule within 0.1%.
  - On a 1080×1920 timeline, Strength 0.4 gives a sigma of 9.46 pixels. The rule gives 9.72.
- **vidcut:** all timelines use the rule. On a 1080×1920 timeline, the blur is thus 3% larger than Resolve's.
- **Why:** the measurements do not explain the 3%. The rule fits each landscape timeline that was measured.

## X103. The render cache is made in the background

- **Resolve:**
  - Resolve caches the parts to cache while they play. It also caches in the background, after a number of seconds without changes (pp. 199–206).
  - Each project has its own cache folder. A preference deletes cache files after a number of days.
  - The project settings hold the render cache format, the background render, its time and the kinds of User mode.
  - The Render Cache Format menu offers ProRes, DNxHR and uncompressed formats (p. 202).
  - Smart caches clips in formats that are slow to decode: H.264, H.265, DCP, JPEG 2000 and camera raw.
  - Smart caches speed effects and Fusion effects. The manual does not name the upscaler or frame rate conversion for Smart.
- **vidcut:**
  - Playback does not write the cache. Only the background render writes it ([D98](spec.md#12-decisions-on-gaps-and-contradictions)).
  - The render cache is outside the cache dir. It does not count in the limit of the disk cache. No preference deletes old files. Delete Render Cache removes them.
  - Preferences hold the codec, the background render, its time and the kinds of User mode. They are the settings of this computer.
  - The codecs are the ProRes and DNxHR grades, HEVC and AV1, and Automatic. There is no uncompressed format.
  - Smart finds the media that are slow to decode by the decoder and by the proxy rules, not by a list of formats:
    - with GPU decoding on, media of a codec with long groups of pictures that the GPU does not decode
    - media that the proxy rules pick (spec D41), while the original plays
  - Smart also renders AI Super Scale, and each Retime Process other than Nearest.
  - Smart also renders the parts where frames drop steadily during play ([D98](spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - The writes of playback would take GPU time from the frames on the screen.
  - The codec, the kinds of User mode and the time depend on the computer, not on the project. Its encoders, its decoders and its disk set them.
  - vidcut decodes H.264 and H.265 on the GPU. With the list of Resolve, Smart renders most clips of a camera or a phone.

## X105. Manage Render Cache lists the projects that vidcut opened

- **Resolve:**
  - Playback > Manage Render Cache shows the render cache of the projects in the project libraries (p. 206).
  - Location selects Local, Network, Cloud or All. Project Library selects one library, or All.
  - Each project has its own row.
- **vidcut:**
  - vidcut has no project libraries. A project is a file.
  - The window lists each project that vidcut opened on this computer. A project whose file is gone is not in the list.
  - Location is Network when a network drive holds the project file. Else it is Local. There is no Cloud.
  - Project Library is the folder that holds the project file.
  - Projects whose render cache is in one folder share a row. Clear Selected Cache clears that folder for all of them.
    - By default, the projects in one folder share a render cache folder ([X26](#x26-one-folder-for-proxies-and-the-render-cache-is-a-preference)).
  - Delete Render Cache > All and Unused also remove the frames of the other projects in that folder.
- **Why:**
  - Without libraries, the projects that vidcut opened are the projects that it knows.
  - The folder of a project file is the nearest thing to a library.
  - The files of a cache folder have names from their content. Thus two projects can share a file, and vidcut can clear only the full folder.

## X106. A clip with an alpha channel is cached with the tracks mixed

- **Resolve:**
  - Uncompressed 10-bit, Uncompressed 16-bit Float, ProRes 4444, ProRes 4444 XQ and DNxHR 444 keep the alpha channel of a cached clip (p. 201).
  - Thus Resolve can cache a clip with an alpha channel at the clip level in these formats.
- **vidcut:**
  - Only ProRes 4444 and ProRes 4444 XQ keep the alpha channel. Media with an alpha channel is at the clip level only with these codecs ([spec D98](spec.md#12-decisions-on-gaps-and-contradictions)).
  - DNxHR 444 keeps no alpha channel. vidcut has no uncompressed codec ([X103](#x103-the-render-cache-is-made-in-the-background)).
  - At the timeline level, the cache holds the frame after the tracks mix. The frame looks the same.
  - The name of such a frame comes from all its layers, not from its time (`Cache::keys`). Thus its cache stays while its layers stay the same.
  - A change on another track at the time of the frame renders it again. A move of the clip renders it again where other frames are under the clip.
- **Why:**
  - The DNxHR encoder of FFmpeg has no pixel format with alpha.
  - The timeline level gives the same picture with one simple path. The cost is only more renders after edits.

## X110. A shrink above 8:1 reads each source pixel

- **Resolve:**
  - The scaling filter reads 16 source pixels across and 16 down at each scale ([spec §13](spec.md#the-scaling-filter)).
  - The formula is measured down to a shrink of 18.6:1.
  - At a shrink of 24:1, one-pixel lines every 3 source pixels read 64, where their mean is 117.3 (measured in Resolve Studio 21.1.2).
- **vidcut:**
  - Up to a shrink of 8:1, vidcut uses the same filter.
  - Above a shrink of 8:1, vidcut first halves the picture until the shrink that remains is 8:1 or less.
- **Why:**
  - Above 8:1, the filter gives some source pixels more weight than others. Thin lines then make bands.
  - A 4K picture with one-pixel lines every 25 pixels has a mean of 70.4. At a shrink of 32:1, the filter alone reads 64 to 84. With the halving, it reads 69 to 71.
  - Timeline thumbnails and storyboards often shrink this far.

## X111. A very negative line spacing keeps all the lines

- **Resolve:**
  - Line spacing moves the lines closer, and lines can overlap (p. 1184).
  - These are measurements in Resolve Studio 21.1.2 from files, with Arial at size 96:
    - At -112, it draws both lines.
    - At -200 and at -500, it draws only the first of two lines.
    - At -200 on the first line alone, or on the last line alone, it also draws only the first line.
- **vidcut:**
  - It draws all the lines at any line spacing.
- **Why:**
  - Lines that disappear are a fault in Resolve. The manual does not name them.

## X112. A title from an OTIO file has no black edge

- **Resolve:**
  - In Resolve Studio 21.1.2, a title read from an OTIO file has a black edge of 1 pixel on each letter. The edge goes over all that is under it.
  - Its letters are 1 pixel thinner, and they sit half a pixel higher.
  - The cause is one stored value: the reader gives each piece of text an outline width of 1.
  - The same value moves the lines `<u>`, `<o>` and `<s>` 2 to 3 pixels down.
  - The same title typed in the Inspector, saved and opened again, or read from a `.drt` file has no such edge.
- **vidcut:**
  - A title draws the same from any source: as a title typed in the Inspector.
- **Why:**
  - The edge is a fault in the reader. The manual does not name it, and the author of the title never set it.

## X113. Source Overwrite starts where the file starts

- **Resolve:**
  - With timeline marks, the clip keeps the marked length before the file's start. These are measurements in Resolve Studio 21.1.2.
  - The clip holds the file's first frame for the frames before the file starts. With marks that end before the file's start, the whole clip is that frame.
  - Its OTIO export does not hold this offset. The OTIO alone thus places the file early.
- **vidcut:**
  - The clip starts where the file starts. It holds no frozen frame.
  - When nothing of the marked range is left, the edit writes nothing ([X114](#x114-source-overwrite-tells-when-it-writes-nothing)).
- **Why:**
  - Source Overwrite makes a synced cutaway. A frozen frame is not in sync with the timeline.
  - The file also keeps its place in an OTIO export.
  - The owner approved this difference.

## X114. Source Overwrite tells when it writes nothing

- **Resolve:**
  - With no overlapping timecode, the edit does nothing (p. 700).
  - These are measurements in Resolve Studio 21.1.2: the timeline, the marks and the playhead do not change, and no message comes. An empty timeline acts the same.
- **vidcut:**
  - The request returns the error `INVALID_EDIT` for the CLI, IPC and MCP.
  - The window shows a short message.
- **Why:**
  - A button that does nothing looks broken.
  - A script needs to know that no clip went in.
  - The owner approved this difference.

## X115. A typed duration gives exactly that length

- **Resolve:**
  - The Duration field takes a typed length, and it then shows that length.
  - The edit from these marks has one frame more. These are measurements in Resolve Studio 21.1.2.
  - `00:00:02:00` gives 51 frames at 25 fps, and 61 frames at 30 fps.
  - The Multi Source grid and Source Clip act the same.
- **vidcut:**
  - The marks hold exactly the typed length.
  - `00:00:02:00` gives 50 frames at 25 fps, and 60 frames at 30 fps.
- **Why:**
  - Two seconds at 25 fps are 50 frames.
  - The field and the edit agree.
  - The owner approved this difference.

## X116. The Ripple Overwrite button treats other tracks as a drop does

- **Resolve:**
  - A drop of a file onto a Track 1 clip follows the rule in [D21](spec.md#12-decisions-on-gaps-and-contradictions). These are measurements in Resolve Studio 21.1.2.
  - The button and Shift+F10 act differently, on the Cut page and the Edit page.
  - They cut each other track at the start of the replaced part. Everything from there on moves by the change in length.
  - With a shorter clip, the frames just before that start also go away. The edit does not replace these frames.
- **vidcut:**
  - The button, the key, the CLI, IPC and MCP follow the rule of the drop.
- **Why:**
  - The button takes out picture that the edit does not replace. It also cuts clips that start before the replaced part.
  - One rule for one edit is easier to predict.
  - The owner approved this difference.

## X117. Cues stay with the picture in a Ripple Overwrite

- **Resolve:**
  - A cue over the replaced clip stays where it is, also past the new end of the clip. These are measurements in Resolve Studio 21.1.2.
  - A later cue that moves back cuts the cue that it lands on. It removes the replaced clip's cues from its landing on.
  - The user then has to re-time the cues.
- **vidcut:**
  - Cues keep the frames of Track 1 under them ([D57](spec.md#12-decisions-on-gaps-and-contradictions)).
  - A cue loses its part over the frames that the edit takes out. A cue with no frames left goes away.
  - A longer clip splits a cue across the end of the replaced part. Both parts keep the text.
  - Later cues move with their picture.
- **Why:**
  - A cue belongs to the picture under it.
  - The user does not have to re-time the cues.
  - The owner approved this difference.
## X118. Source Overwrite at the end of the timeline uses the first clip

- **Resolve:**
  - The playhead is at the end of the timeline, after the last clip. The last clip shows part of the marks.
  - With a last clip of 139 frames or less, the reference is that last clip.
  - With a last clip of 140 frames or more, the reference is the first clip that shows part of the marks.
  - These are measurements in Resolve Studio 21.1.2, in the Multi Source viewer, with one layout. The reason for the limit is not known.
- **vidcut:**
  - With no clip under the playhead, the reference is the first clip that shows part of the marks. The length of the last clip plays no part.
- **Why:**
  - A limit with no known reason looks like a fault, and the user cannot predict it.
  - The owner approved this difference.

## X119. Auto Sync on a saved group waits for Save Sync

- **Resolve:**
  - These are measurements in Resolve Studio 21.1.2. Auto Sync in an opened group replaces the pool item at once.
  - The clip on the timeline goes offline, and it stays offline after Save Sync.
- **vidcut:**
  - Auto Sync changes only the places in the window.
  - Save Sync writes them to the same group. Cancel loses nothing.
- **Why:**
  - The offline clip is a fault. A sync must not break a timeline.
  - The owner approved this difference.

## X120. A drag in an opened group changes the group at Save Sync

- **Resolve:**
  - This is a measurement in Resolve Studio 21.1.2. A drag in an opened group changes the pool item at once, before Save Sync.
- **vidcut:**
  - A drag changes only the places in the window. Save Sync writes them to the group.
- **Why:**
  - The end result is the same.
  - The history gets one entry, and Cancel has nothing to undo.
  - The owner approved this difference.

## X121. The Sync Clips window is inside the main window

- **Resolve:**
  - This is a measurement in Resolve Studio 21.1.2. The icon opens a separate window with the title "Sync Clips".
- **vidcut:**
  - The window is inside vidcut's main window.
- **Why:**
  - The window is for one short task, so it needs no place of its own on the screen.
  - The owner approved this difference.

## X122. A clip on the timeline does not follow Save Sync

- **Resolve:**
  - This is a measurement in Resolve Studio 21.1.2. After a drag and Save Sync, the multicam clip on the timeline follows the group.
- **vidcut:**
  - A clip already on the timeline stays as it is.
- **Why:**
  - vidcut has no clip of a group on the timeline. Source Overwrite makes clips of one file.
  - The owner approved this difference.

## X123. The Sync Clips icon opens a saved group

- **Resolve:**
  - These are measurements in Resolve Studio 21.1.2. The icon always starts a new group from the timecodes, and the old group stays in the pool.
  - A saved group plus one new file gives a new group of all the files, at their timecodes.
  - Thus the icon opens all the files as a new sync. This brings a file dropped by a drag back and loses the saved places.
- **vidcut:**
  - The icon on files that are already one saved group opens that group.
  - A file that the group dropped does not count as new, so the icon opens the saved group.
- **Why:**
  - Two groups on one file leave the older one hard to reach.
  - The icon keeps the saved work when it opens the saved group.
  - The owner approved this difference.

## X125. Sync by Sound uses time order on a row

- **Resolve:**
  - This is a measurement in Resolve Studio 21.1.2. Sync by Sound takes the files of a row in bin order.
  - With c2 first in the bin and c1 second, it drops c1 silently, although c1's frames 0 to 375 have nothing over them.
- **vidcut:**
  - Files on a row go by their place from the sound. A tie goes by bin order.
  - A file keeps only the part after everything before it. A file with no such part is not on the row.
  - Thus c2 and c1 give the same result as c1 and c2: c1 at 0 whole, c2 at 500 with head 125.
- **Why:**
  - Nothing is lost for a reason the user cannot see, and the result does not depend on bin order.
  - This is the rule of Timecode, so one rule serves both.
  - The owner approved this difference.

## X126. Source Overwrite keeps sync with a timeline Out alone

- **Resolve:**
  - This is a measurement in Resolve Studio 21.1.2. With an Out mark and no In on the timeline, the clip starts at the timeline's first frame with the file's first frame.
  - The clip is not in sync, also when a clip with the same timecode is under it. This looks like a fault.
- **vidcut:**
  - The reference is the first clip before Out with shared timecode.
  - The new clip goes in sync from where the file starts to Out.
- **Why:**
  - Source Overwrite makes a synced cutaway. A clip that is out of sync is the fault that this edit is there to prevent.

## X128. Removing a file takes it out of its groups

- **Resolve:**
  - This is a measurement in Resolve Studio 21.1.2. A file deleted from the Media Pool stays in its group. The group keeps its 3 angles.
- **vidcut:**
  - `media.remove` also takes the file out of its groups. A group with no file left goes too.
- **Why:**
  - A group in vidcut holds only files that are in the project.
  - The owner approved this difference.

## X129. Generators have the correct picture where Resolve has a fault

- **Resolve:**
  - These are measurements in Resolve Studio 21.1.2. Both faults repeat.
  - In an HLG timeline, the YCbCr Ramp has green × 1.0634 for x 0 to 204 at 1920 wide. This gives a step at x 205.
  - At an odd size (1001 × 777), column 0 has noise in red on the BT.2111 Color Bars and SMPTE Color Bar. The other kinds have extra colors there.
- **vidcut:**
  - The YCbCr Ramp has no step. Its code values are the same in each timeline.
  - Each kind follows its rule at each size, in each column.
- **Why:**
  - These are faults in Resolve. vidcut has the correct picture.

## X130. A camera opened alone keeps the length of the grid marks

- **Resolve:**
  - These are measurements in Resolve Studio 21.1.2.
  - In the Multi Source grid, an In at 250 and an Out at 350 give 101 frames.
  - Opening a camera alone gives the file an In at 250 and an Out at 349. An edit then takes 100 frames.
- **vidcut:**
  - Opening a camera alone gives the file the marks of the grid as they are.
  - An edit from the grid and an edit from the camera alone take the same frames.
- **Why:**
  - The same marks give the same frames in both views.
  - The owner approved this difference.

## X131. A broken `.cube` file is refused

- **Resolve:**
  - These are measurements by script in Resolve Studio 21.1.2.
  - `SetLUT` returns True on each of these files:
    - A lower-case size line gives no LUT.
    - A file with no size line gives no LUT.
    - Too few rows give a wrong picture.
    - `LUT_3D_SIZE 1` gives a black picture.
  - The window of Resolve is not measured.
- **vidcut:**
  - `media.set` and `project.set` refuse the file and give the reason.
- **Why:**
  - An error is better than a wrong picture that looks correct.
  - The owner approved this difference.

## X132. A Mosaic Blur Triangle keeps its own color in a corner at the side of the frame

- **Resolve:**
  - These are measurements in Resolve Studio 21.1.2. The manual does not tell this.
  - A Triangle has corners at the sides of the frame, from triangles with a middle off the frame.
  - When that middle is 0.691 of a column step or more off the frame, the corners take other colors.
  - Two renders of Aspect 2 at Pixel Frequency 25 differ in these corners.
- **vidcut:**
  - A corner has the box at the middle of the pixel's own triangle, clamped into the frame, at each distance.
- **Why:**
  - The Resolve result does not repeat.
  - The owner approved this difference.

## X133. A Mosaic Blur cell edge exactly on a pixel gives the pixel to the cell on the right or below

- **Resolve:**
  - These are measurements in Resolve Studio 21.1.2. The manual does not tell this.
  - Where a cell edge falls exactly on a pixel, Resolve gives that pixel to either cell. The renders show no rule.
- **vidcut:**
  - That pixel goes to the cell on the right or below.
- **Why:**
  - vidcut has one fixed rule where the renders show none.
  - The owner approved this difference.

## X134. With FFmpeg 8, vidcut refuses an animated WebP

- **Resolve:**
  - Resolve imports an animated WebP.
- **vidcut:**
  - With FFmpeg 8, vidcut refuses an animated WebP on import. The message says that the file needs FFmpeg 9.0.
- **Why:**
  - FFmpeg 8 reads the file as a still of 2×2 pixels, and its picture never renders ([gotchas](gotchas.md#ffmpeg-reading-and-decoding)).
  - A refusal gives the reason. A still that shows nothing does not.
  - The owner approved this difference.

## X135. One key has one action

- **Resolve:**
  - These are measurements in Resolve Studio 21.1.2. The manual tells the warning and the badge (p. 125).
  - A warning names each command that has the key.
  - Assign keeps the key on all these commands, with a warning mark on each row.
  - At most one command runs on a press.
- **vidcut:**
  - vidcut refuses a key that another action uses. The refusal names that action.
  - The user gives that action another key first.
- **Why:**
  - One key on two actions is confusing.
  - The rule that selects the action in Resolve is not known.
  - The owner approved this difference.

## X136. DNxHR 444 exports without alpha

- **Resolve:**
  - Export Alpha works with DNxHR 444 (p. 4093).
- **vidcut:**
  - `video.alpha` refuses DNxHR 444. The Export dialog does not show Export Alpha for it.
- **Why:**
  - The FFmpeg encoder `dnxhd` cannot write alpha.
  - The owner approved this difference.

## X137. A Scroll line wider than the picture wraps

- **Resolve:**
  - A line of a Scroll that is wider than its text box stays on one line. The box is as wide as the picture.
  - The picture cuts the line at its edges, and those letters are lost (measured in Resolve Studio 21.1.2).
- **vidcut:**
  - The line wraps at 90% of the picture width, as the lines of other titles do.
- **Why:**
  - A wrapped line loses no letters.
  - The owner approved this difference.

## X138. The keys are a page of Preferences

- **Resolve:**
  - Keyboard Customization is a separate window (p. 121).
- **vidcut:**
  - The keys are the Keys page of the Preferences window.
- **Why:**
  - vidcut stores the keys in the preferences, with the other settings of the user.
  - The owner approved this difference.

## X139. One Input LUT submenu takes any `.cube` file

- **Resolve:**
  - A right-click on Media Pool clips gives the 1D LUT and 3D LUT submenus. The user selects a LUT there (p. 3480).
  - The submenus list the LUT files in the LUT folders that the preferences name (p. 3480).
- **vidcut:**
  - The right-click menu of the pool has one Input LUT submenu, with Choose… and Reset.
  - Choose… opens a file picker for any `.cube` file.
- **Why:**
  - The picture is the same.
  - The picker reaches any file, with no LUT folder to set up.
  - The owner approved this difference.

## X140. The Atmosphere noise of a Glow has its own pattern

- **Resolve:**
  - These are measurements in Resolve Studio 21.1.2 (Glow, p. 3590).
  - Atmosphere multiplies the glow by a noise.
  - The renders show the statistics and the size of the noise at 1920×1080. They do not show the rule that makes its pattern.
- **vidcut:**
  - vidcut makes the noise itself ([spec §8.16](spec.md#816-effects-pp-765-3526-3536)).
  - The noise has the statistics of the renders at the measured values. Its pattern is not the pattern of Resolve.
- **Why:**
  - No render shows how Resolve makes the pattern. Thus no frame can be the same.
  - The owner approved this difference.

## X141. A Glow up to a Spread of 0.1 has a plain blur

- **Resolve:**
  - These are measurements in Resolve Studio 21.1.2 (Glow, p. 3590).
  - At a Spread of 0.02, 0.05 and 0.1, the blur has steps of 8 pixels. The renders do not show the rule of the steps.
- **vidcut:**
  - The blur is the plain Gaussian curve at these values.
  - Against the renders, the largest difference is 0.0099, at a Spread of 0.05. The rms difference is about 0.0002.
- **Why:**
  - No rule for the steps is known. The difference is small.
  - The owner approved this difference.

## X142. A Glow keeps the tracks below around a smaller picture

- **Resolve:**
  - These are measurements in Resolve Studio 21.1.2 (Glow, p. 3590), with Use Alpha on, as an effect added in the GUI has it.
  - The output of a Glow has an alpha of 1, also outside a smaller picture and in the clear parts of the picture.
  - Thus a Glow on a clip at Zoom 0.5 makes the frame black around the clip. The tracks below do not show.
- **vidcut:**
  - The alpha of the output is the larger of the picture's alpha and the brightest color of the output.
  - Other tracks below show around the clip and through its clear parts. The glow around the clip covers them by its brightest color.
- **Why:**
  - The black is a fault in Resolve. An effect on a clip does not hide the tracks below.
  - The owner approved this difference.

## X143. The noise of a Film Grain has its own pattern

- **Resolve:**
  - These are measurements in Resolve Studio 21.1.2 (Film Grain, p. 3559).
  - The renders show the spectrum and the statistics of the noise. They do not show the rule that makes its pattern.
- **vidcut:**
  - vidcut makes the noise itself from the measured spectrum ([spec §8.16](spec.md#816-effects-pp-765-3526-3536)).
  - The noise has the spectrum and the statistics of the renders. Its pattern on each frame is not the pattern of Resolve.
- **Why:**
  - No render shows how Resolve makes the pattern. Thus no frame can be the same.
  - The owner approved this difference.

## X144. A Film Grain has no jumps at Symmetry 0.4 and 0.6

- **Resolve:**
  - These are measurements in Resolve Studio 21.1.2 (Film Grain, p. 3559), with Grain Only and Softness 0, at each 0.02 of Symmetry.
  - At exactly 0.4 and 0.6, the standard deviation of the noise is 0.0109 and 0.0855. At the values on each side, it is about 0.026.
- **vidcut:**
  - Symmetry follows a table of the measured curves and blends between them ([spec §8.16](spec.md#816-effects-pp-765-3526-3536)).
  - The table has no curve at 0.4 and 0.6. There, vidcut blends the curves at 0.38 and 0.42, and at 0.58 and 0.62.
- **Why:**
  - The jumps are a fault in Resolve. A small change of Symmetry does not make a large change of the grain.
  - The owner approved this difference.

## X146. Straight ProRes 4444 holds straight colors

- **Resolve:**
  - With Straight, a ProRes 4444 file still holds premultiplied colors (measured in Resolve Studio 21.2). ProRes 4444 XQ is not measured.
- **vidcut:**
  - With `video.alpha_mode` `straight`, ProRes 4444 and 4444 XQ hold straight colors.
- **Why:**
  - The file holds what its Alpha Mode says. The file from Resolve does not agree with its own setting.
  - The owner approved this difference.

## X147. An image sequence refuses sound settings

- **Resolve:**
  - With TIFF and OpenEXR, the Audio tab is greyed out.
  - A render from the scripting API with Export Audio on writes only the frames (measured in Resolve Studio 21.2).
- **vidcut:**
  - A TIFF or OpenEXR export with `audio` settings stops with an error. It writes no files.
- **Why:**
  - The files cannot hold sound. vidcut does not drop a setting without a word.
  - The owner approved this difference.

## X148. An OpenEXR in drop-frame timecode has the drop-frame flag

- **Resolve:**
  - In drop-frame timecode, the frame digits of `timeCode` skip the dropped numbers. The drop-frame flag (bit 6) is 0.
  - This is so at 29.97, 59.94 and 119.88 fps (measured in Resolve Studio 21.2).
- **vidcut:**
  - The frame digits are the same. vidcut sets the drop-frame flag.
- **Why:**
  - The `timeCode` of OpenEXR follows SMPTE 12M. There, bit 6 is the drop-frame flag.
  - Without the flag, a reader takes the digits as non-drop-frame. At 29.97 fps, a frame count from them is then 2 frames too high for each minute that drops numbers.
  - The owner approved this difference.

## X149. A project takes any frame rate

- **Resolve:**
  - A project takes only the 19 frame rates of spec D37.
  - The scripting API refuses other rates, for example 15, 20, 144 and 240 (`timelineFrameRate`, measured in Resolve Studio 21.2).
- **vidcut:**
  - The Frame rate menu offers the same 19 rates.
  - `project create --fps` and the first video can set any positive rate.
- **Why:**
  - A file at another rate keeps each frame at its own rate. Examples are a 15 fps screen recording and a 144 fps game capture. A project at one of the 19 rates retimes such a file.
  - The owner approved this difference.

## X150. A subtitle shadow moves right by its full offset

- **Resolve:**
  - These are measurements in Resolve Studio 21.2, on 1920×1080 and 3840×2160 timelines.
  - A positive X offset of a subtitle shadow moves it 1 pixel less than the Inspector shows. An offset of 40 moves it 39 pixels, and an offset of 2 moves it 1 pixel.
  - A negative X offset moves it by the value that the Inspector shows, within 0.1 pixel.
- **vidcut:**
  - The shadow moves by the value that the Inspector shows, to the left and to the right.
- **Why:**
  - Only a move to the right is short. Thus the short move is not a rule of the offset, and vidcut does not copy it.

## X151. An OpenEXR with DWAA or DWAB holds its level

- **Resolve:**
  - The header of an OpenEXR with DWAA or DWAB has no `dwaCompressionLevel` (measured in Resolve Studio 21.2).
- **vidcut:**
  - The header has `dwaCompressionLevel`, the Compression Level of the export.
- **Why:**
  - The OpenEXR writer of vidcut writes this attribute for each level that it gets.
  - The attribute is part of the OpenEXR format. A reader can see the level of the file, and a reader that does not know it skips it.

## X152. Effect keyframes have one number each

- **Resolve:**
  - Mosaic Blur Position and Vignette Center are one point with X and Y (measured in Resolve Studio 21.2).
  - Film Grain Freeze takes a curve (measured in Resolve Studio 21.2).
- **vidcut:**
  - Position X, Position Y, Center X and Center Y have keyframes of their own.
  - Film Grain Freeze is a switch and has no keyframes.
- **Why:**
  - The keyframes of an effect are one list of numbers for each value. A pair of numbers needs a second list, as Transform has, for two values only.
  - A keyframe is a number on a curve. A switch has no value between on and off, and the curves of vidcut have no steps.

## X153. Every clip pans as on a stereo track

- **Resolve:**
  - The law of the Pan is different on a stereo track and on a mono track (measured in Resolve Studio 21.2).
  - The [spec](spec.md#9-audio-pp-642-649650-743744-771-773-778779) gives both laws.
  - The scripting API takes a Pan outside −100 to 100, and sets the nearest limit.
- **vidcut:**
  - A track has no channel type. Each clip uses the law of a stereo track.
  - A mix of one channel has no sides. The Pan does not change it.
  - A mix of three channels or more applies the Pan to its first two channels.
  - `clip.set` refuses a Pan outside −100 to 100.
- **Why:**
  - Resolve puts a mono clip on a stereo track by default. Thus the law of a stereo track is what most clips get.
  - A value outside the range is an error in the script. A refusal shows the error.

## X155. The Dialogue Leveler follows the levels with its own rules

- **Resolve:**
  - The Dialogue Leveler finds dialogue with its own analysis (pp. 774–775). Its rules are not public.
  - The [spec](spec.md#9-audio-pp-642-649650-743744-771-773-778779) gives what the renders show.
  - The scripting API sets an Output Gain outside 0 to 6 dB to the nearest limit.
  - It does not work on a track with more than two channels (p. 775).
- **vidcut:**
  - It does not separate dialogue from other sound. A part that is 10 dB above the lowest recent level is dialogue.
  - Background Reduction lowers all the sound between lines. The amount is 1.2 to 5 dB for each preset.
  - On the spoken test clip with Allow Wider Dynamics, each line is within 1.3 dB of Resolve's level. With the other presets, a line is up to 5.1 dB away.
  - An OTIO export keeps it in vidcut's own metadata. The export gives the note `dialogue_leveler_kept`.
  - `clip.set` refuses an Output Gain outside 0 to 6 dB.
  - It works on a clip with any number of channels.
- **Why:**
  - The player runs the same code as the export. Level rules with a short look ahead run in real time.
  - Background Reduction in the renders was uneven. A fixed amount is predictable.
  - A value outside the range is an error in the script. A refusal shows the error.
  - A track of vidcut has no channel type. Thus a clip with many channels gets what the user asks for.

## X156. The Compressor is on a clip

- **Resolve:**
  - The Compressor is a module of the Dynamics of a track (pp. 3946–3947).
  - It has a sidechain and a graph (pp. 3946–3947).
- **vidcut:**
  - The Compressor is a setting of a clip. A window from the Audio section sets it.
  - It has no sidechain and no graph.
  - An OTIO export keeps it in vidcut's own metadata. The export gives the note `compressor_kept`.
- **Why:**
  - The tool of the Audio section acts on the selected clips, as the Equalizer does.
  - A clip recorded at its own level needs its own settings.

## X157. Noise Reduction is our own spectral gate

- **Resolve:**
  - Noise Reduction is a Fairlight FX with its own analysis (pp. 4041–4042).
  - Its scripting API cannot set its values. Thus its curves and its ranges are not measured.
- **vidcut:**
  - Each frequency bin has a signal profile and a noise profile.
  - A bin below the Threshold over its noise profile loses level. A bin at the noise level loses the Threshold and the Sensitivity.
  - It has no Listen To Noise Only, no presets and no spectrum graph.
  - A steady tone is noise to it, as it has no steady level above its own minimum.
  - At the default values, a spoken clip with noise at −60 dBFS gives this result:
    - The noise between the lines loses 12.5 dB. In the first gap it loses 8.8 dB, as the profile is still new.
    - Lines from −16 to −44 dBFS lose 0.0 to 0.7 dB.
  - An OTIO export keeps it in vidcut's own metadata. The export gives the note `noise_reduction_kept`.
- **Why:**
  - The effect has no public rules. A spectral gate is the usual way to lower steady noise and it runs in real time.
  - The default effect lowered the noise by 0.4 dB and the quiet lines by up to 6.0 dB (spec). Meeting recordings need the opposite.

## X160. The graph sits above the tracks

- **Resolve:**
  - The Curve Editor takes the place of the tracks. One keyframe track for the selected curve is at its bottom (p. 1261).
  - A Parameters menu with checkboxes selects the curves (p. 1262).
  - After a click on an empty part of the graph, no curve is active (measured in Resolve Studio 21.2).
  - A larger editor is a window of its own (p. 1260).
- **vidcut:**
  - The Graph button shows the graph above all the tracks. The tracks stay under it.
  - The curve button of each track turns its line on or off in the graph.
  - One curve is always active. A click on an empty part of the graph clears only the selection.
  - A drag on the line under the graph changes its height. No window opens.
- **Why:**
  - The tracks keep the diamonds of all the values. A box and a group drag still work there.
  - With one active curve, its points and handles stay ready to use.
  - Each track already has a curve button. Checkboxes in a menu repeat it.

## X161. Alt moves a handle alone, and the other handle stops at a keyframe

- **Resolve:**
  - Alt and a drag on a Bezier handle moves nothing (measured in Resolve Studio 21.2).
  - The other handle keeps its length on the screen (measured in Resolve Studio 21.2). Its limit at the next keyframe is not measured.
- **vidcut:**
  - With Alt, the handle moves alone. The other handle of the keyframe stays.
  - The other handle stops at the next keyframe on its side.
  - A first or a last keyframe has nothing to line up.
- **Why:**
  - A corner in the curve is then a choice. Alt has no other use on a handle.
  - A curve has one value at each frame. A handle past the next keyframe would break that.

## X163. A compound clip takes its clips whole

- **Resolve:** ([spec §8.18](spec.md#818-compound-clips-pp-10521054-1126-measured-in-resolve-studio-2120))
  - The window of New Compound Clip has a name and a start timecode (p. 1052).
  - A selection can hold only the picture or only the sound of a linked clip.
  - A selected item on a locked track stays out, and the other items go in.
  - Convert In and Out to Compound Clip ignores a lock. It cuts a locked track and takes its clips inside.
  - A range with no clips does nothing, and shows no message.
  - A dissolve between two selected clips goes inside only when the dissolve is also selected. It then plays inside.
  - Decompose in Place has a submenu: Using Clips Only and Preserving Audio Data…
- **vidcut:**
  - The window has only a name.
  - A compound clip in the Media Pool does not open. Its menu has Append to Timeline, Place at Playhead and Delete.
  - A clip goes in with its picture and its sound.
  - A clip on a locked track refuses the request. In Convert In and Out to Compound Clip, a clip in the range on a locked track refuses it too.
  - A range with no clips refuses the request, with a message.
  - A J or L cut that reaches a mark is undone at its picture cut, as at a blade.
  - A dissolve goes inside when its clips go inside. It plays there.
  - `edit.decompose` acts as Using Clips Only. It brings such a dissolve back.
  - After a decompose beside a J or L cut, the cut stays. A sound that comes back starts after that cut.
    - A clip whose sound the cuts cover, alone or together, has no sound.
- **Why:**
  - A project has one timeline (X2). Thus a pool item has no timeline to open.
  - A clip holds its picture and sound as one. Half of a clip is not a thing that vidcut can select.
  - A locked track must not change. A selection that goes in only partly is a surprise. A refusal shows the lock.
  - As in [X27](#x27-a-dissolve-short-of-media-is-shortened), a stop without a message makes the user guess.
  - A sound that runs past a mark cannot stay on the timeline without its picture.
  - vidcut has no selection of a dissolve in this request. A dissolve that goes away is a loss that the user did not ask for.
  - The behavior of Preserving Audio Data is not known.
  - The main track plays one sound at a time. Thus a sound that comes back cannot play under a J or L cut.

## X164. A mode inside a compound clip always mixes with the tracks below

- **Resolve:** (measured in Resolve Studio 21.2.0)
  - One picture in a compound clip mixes its composite mode with the tracks below ([spec](spec.md#12-decisions-on-gaps-and-contradictions)).
  - In one case the mode mixes with black. The compound clip has its usual values, and the picture has an opacity of 100%.
  - In that case Multiply gives a black frame. Screen, Add and Difference give the picture with no change.
  - Each of these changes brings back the rule of the spec:
    - An opacity of 99% on the compound clip.
    - A pan of 1 pixel, a zoom of 0.99, a crop of 1 pixel or a rotation of 0.1°.
    - An opacity of 50% on the picture inside.
  - A title alone does not act in this way. Its mode mixes with the tracks below.
- **vidcut:**
  - The mode of one picture always mixes with the tracks below.
- **Why:**
  - A move of 1 pixel must not change a black frame to the correct picture. This behavior of Resolve is a fault.

## X165. A clip has motion paths

- **Resolve:**
  - The Cut page makes no track of a thing in the picture. Its chapters name the Tracker palette only under Stabilization (pp. 738, 770).
  - The Tracker palette of the Color page moves a window or an effect with a feature of the frame (pp. 3308, 3494).
  - Its tracking data is apart from the values of the window, and those values can have keyframes (p. 3309).
  - On a frame with no tracking data, the window stays where the first or the last tracked frame put it (p. 3310).
    - For a subject that disappears, the manual tells the user to fade the correction out with keyframes (p. 3310).
  - Magic Mask takes a stroke out of the tracking at the frame where its subject is fully off screen (p. 3305).
  - The scripting API of Resolve Studio 21.2 has no call that makes or reads a track.
- **vidcut:**
  - A clip of footage, a still or an image sequence has motion paths. A motion path tells where one area of the source picture is on each source frame.
  - `motion.add`, `motion.set`, `motion.remove` and `motion.list` write and read the paths. A script gives the samples.
  - A path that `motion.track` made holds its place outside its samples. Before its first sample and after its last, the area is where that sample puts it.
  - Another path lasts from its first sample to its last. Before and after them it has no area. Thus a path that `motion.find` made ends where its thing is gone.
  - The area of an effect can follow a path ([X166](#x166-an-effect-has-an-area)). `motion.find` makes a path for each face or each block of text ([X167](#x167-vidcut-finds-the-faces-and-the-text-of-a-clip)).
  - The Inspector has a Motion section for a clip that takes paths, above the effects. Its first row has a Subject menu, Find and Blur.
- **Why:**
  - A blur over a face or a number plate must follow it. A cut that vidcut delivers does not go through another page.
  - The path is apart from the keyframes, as the tracking data of the Color page is. Thus a new path keeps the keyframes that the user set.

## X166. An effect has an area

- **Resolve:**
  - A search of the Cut page chapters (pp. 635–791) finds no window and no mask for an effect of a clip.
  - The Color page has Power Windows: Circular, Linear, Polygonal, Curve and Gradient (p. 3308).
  - A Polygonal window starts as a rectangle with four corner points. A click on it adds a control point, up to 128 (p. 3272).
  - A key adds a node with a window (p. 3268). A Circular, Linear or Polygonal window starts in the middle of the picture, with its controls in the Viewer (21.2).
  - A Curve window starts with no shape. Each click in the Viewer adds a point, and a click on the first point closes the shape (pp. 3273–3274).
  - A polygon window can be animated with the Keyframe Editor (p. 3273).
  - The blur of the node is in the Blur palette. It shows only inside the windows of the node (21.2).
  - The Viewer shows the outline of a window only while the Window palette or the Tracker palette is open (21.2).
    - With the Blur palette open, it shows no outline. The Cut page shows no outline, and nothing there marks a clip with a window.
- **vidcut:**
  - An effect of a clip can have an area: an ellipse, a box or a polygon. The effect then shows only inside it. With `invert`, it shows only outside it.
  - The area stays on the content of the picture through the transform, the crop, the flips, a Dynamic Zoom and a compound clip around it.
  - With `follows`, the area follows a motion path of the clip ([X165](#x165-a-clip-has-motion-paths)).
    - `follows` can also name a subject, as `face`. The effect then works in the area of each path of that subject.
    - With `invert`, the effect works outside each area. While no path has an area, it then works on the whole picture.
    - `motion.remove` refuses a path that an area follows by its id. A path that an area follows by its subject can go.
  - Each effect in the Inspector has Area rows: Off, Ellipse, Box or Polygon, then Edit Shape, Track, Invert Area, Area Softness and Follow.
    - Area Advanced Controls has Center X, Center Y, Width, Height and Rotation.
  - The viewer shows a thin outline of each area of the selected clip at the playhead.
    - A click on an outline, or inside it, shows the handles of that area. A click outside the areas hides the handles.
    - With no clip selected, the viewer shows no outline. Thus it shows the picture of an export.
  - Edit Shape shows the handles of the area in the viewer.
    - A drag inside the area moves it. On an ellipse or a box, a corner changes its size, and the top handle turns it.
    - On a polygon, Alt and a click also removes a point. Why: a trackpad has no middle button.
  - The Tools row under the viewer has Blur ([spec §8.15](spec.md#815-transform-and-crop-pp-766768)).
    - The right-click menu of a clip has Blur. It shows the Tools row with Blur on, for that clip.
    - With Ellipse or Box, a drag in the picture adds a Mosaic Blur with that area. With Shift, the area is as wide as it is high on the screen.
    - With Free Shape, each click adds a point. A click on the first point, or a double-click, closes the shape. Escape drops it.
    - A drag on the outline of another area shows its handles and moves it.
  - The points of a polygon take no keyframes.
- **Why:**
  - Keyframes on the points of a polygon are not built yet ([todo](todo/motion-tracking.md)).
  - The user draws the area on the thing to hide, and sees the blur at once. Resolve needs a node, the Window palette and the Blur palette for that.
  - The outline of an area tells where an effect works, with no tool open.
  - A blur over a face or a number plate must cover only that part of the picture.
  - One effect can cover each face of a clip. The user then changes one blur, not one for each face.
  - One rule gives each of the six effects an area.

## X167. vidcut finds the faces and the text of a clip

- **Resolve:**
  - The Close Up edit of the Cut page does a face detection, and puts a face that it finds in the frame (p. 699).
  - On the Media page, Analyze Clips for People finds faces to make People keywords (p. 418).
  - Face Refinement is a Resolve FX of Resolve Studio. Detect Faces in Frame draws a box on each face (p. 3615).
    - The user chooses one face. The tracking controls of the effect then follow it through the clip (pp. 3615–3616).
  - The manual names no way to make tracking data for each face of a clip on the Cut page ([X165](#x165-a-clip-has-motion-paths)).
  - A search of the whole manual finds no tool that finds text in a picture.
    - A number plate is only an example for the Planar Tracker node of the Fusion page, where the user sets the track (p. 2820).
- **vidcut:**
  - `motion.find` finds the things of one subject on the source frames that a clip shows. The subjects are `face`, `text`, `person` and `vehicle`.
    - A vehicle is a bicycle, a car, a motorcycle, a bus, a train or a truck.
    - The app has buttons for `face` and `text` only.
  - Each thing gets a motion path of its own.
  - A shot change inside a clip ends each path. For a file with another frame rate than the project, `motion.find` and Detect Scene Cuts can find different shot changes.
  - The text model sees the picture with a longer side of 960 pixels. Thus it does not find very small text.
  - While the Inspector is open, the viewer shows a box with its name on each path of the selected clip.
  - After a Find, the status line tells how many things it found.
- **Why:**
  - A blur must cover each face of a clip, and the user must not draw each one.
  - Text is the next thing that people hide: names, addresses, documents and screens.
  - One request gives the paths. The area of an effect then follows a path ([X166](#x166-an-effect-has-an-area)).

## X168. The area of an effect follows a thing

- **Resolve:**
  - The Cut page has no tracker for an effect of a clip. The Tracker palette of the Color page follows a thing with a window (p. 3308).
  - One click on Track Forward tracks from the playhead. The track holds Pan, Tilt, Zoom, Rotate and Perspective 3D (pp. 3312–3313).
  - The tracker does not stop at a shot change. In a clip with a cut, it ran 7 frames into the next shot, and then stopped (21.2).
  - After the track stops, the window stays at its last place for the rest of the clip (21.2).
  - One button tracks forward and reverse from the playhead. A second track from a later frame keeps the track before that frame (21.2).
  - Tracking data is apart from the transform of a window, which can have keyframes. A change of the window offsets it from the tracked path (p. 3309).
- **vidcut:**
  - `motion.track` follows the thing under the area of an effect, backward and forward from one frame.
  - Track follows the thing under an area. It is in the Area rows of each effect, and under the viewer while the handles of an area show.
    - A second Track keeps the track before the playhead, and goes forward from the playhead.
    - On the first frame of a track, Track goes backward, and keeps the track after that frame.
  - The track stops at a shot change. After the track, the area stays at its last place for the rest of the clip. Before the track, it stays at its first place.
  - The track follows the place and the size of the thing. It does not follow a turn or a change of perspective.
  - `motion.track` refuses an area with keyframes on its center, its size or its rotation.
- **Why:**
  - A track that keeps those keyframes as an offset is not built yet ([todo](todo/motion-tracking.md)).
  - A blur must not go on a thing of the next shot.
  - The model gives a box, and a box has no turn.

## X169. A title follows a motion path

- **Resolve:**
  - The Cut page and the Edit page cannot make a title follow a thing. Their chapters name a tracker only for stabilization (pp. 738, 770, 923, 1209).
  - On the Fusion page, a text whose Center connects to a tracker goes onto the tracked path. The X and Y Offset of the tracker move it away (pp. 1793, 1804, 1805).
  - On a frame with no tracking data, a tracked window stays where the first or the last tracked frame put it (p. 3310).
  - The manual does not tell what a connection does to a Center with keyframes. It also does not tell where a node goes when its connection is removed.
- **vidcut:**
  - `motion.follow` makes a title follow a motion path of another clip of the timeline ([X165](#x165-a-clip-has-motion-paths)). Follow in the Inspector of a title lists the paths of the clips under it.
  - The title goes onto the thing: its position becomes the middle.
  - `motion.follow` refuses a title with keyframes on its position.
  - A title that stops following keeps its numbers. Thus it leaves the thing.
  - The title keeps its size and its angle.
  - Before the first sample of the path and after its last, the title stays where that sample puts it.
  - Before and after the clip of the path, the title stays where the first or the last frame of that clip puts it.
  - A scroll follows no path. A title inside a compound clip follows none.
  - An OpenTimelineIO file holds the own position of the title. An import does not read what the title follows.
- **Why:**
  - A name or a label must stay with a person or a thing that moves.
  - One setting does the work of a tracker node and a connection.
  - A title with keyframes, and a title that stops following, wait for a measurement in Resolve ([todo](todo/motion-tracking.md)).

## X170. The clip Equalizer differs in three ways

- **Resolve:**
  - The [spec](spec.md#9-audio-pp-642-649650-743744-771-773-778779) gives the ranges, defaults and curves of the clip equalizer.
  - The curve shows a Notch filter, but a render does not change the sound.
  - A Bell filter is wider than its Q near the top of the range.
  - Q 1 acts as Q 0.94 at 8000 Hz and as Q 0.75 at 16000 Hz. Q 4 acts as Q 2 at 16000 Hz.
  - An OTIO export writes the effect `Fairlight Clip Equaliser` and six band effects.
- **vidcut:**
  - A Notch filter removes its frequency. Its width is fixed.
  - A Bell filter has the given Q at each frequency.
  - An OTIO export keeps the Equalizer in vidcut's own metadata. The export gives the note `equalizer_kept`.
- **Why:**
  - A Notch filter that shows a cut and does not make it is a fault.
  - The measurements do not give one rule for the width of a Bell filter near the top of the range. The difference is small below 8000 Hz.
  - The Equalizer effects of Resolve in OTIO are open work ([todo](todo/effects-library.md)).
