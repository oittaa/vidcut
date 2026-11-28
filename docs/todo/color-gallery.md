# The Gallery, stills, groups and Apply Grade

**When:** now. The owner picked this work on 2026-11-26.

The Gallery of the Color page keeps stills of graded frames. A still holds a picture and a grade. Apply Grade gives a grade to other clips.

A group gives a set of clips two shared grades: one before their clip grades and one after them.

vidcut has the Gallery with stills, Delete Selected and Apply Grade. It has no albums, memories, wipes or groups.

This file tells what Resolve does, what vidcut has, the build steps in order of value, and the Resolve runs of each step.

## What Resolve does

The pages are of the DaVinci Resolve 21.1 manual.

### The Gallery and stills (pp. 3050, 3332–3341)

- The Gallery is at the left of the viewer on the Color page. It shows the stills of the open album and the Memories (p. 3050).
- A still holds a picture of the frame and the grade of the clip (p. 3332).
- Color > Stills > Grab Still (Option-Command-G) saves a still. The menu of the viewer has Grab Still too (p. 3333).
- Grab All Stills and Grab Missing Stills save a still of each clip, from its first or its middle frame (p. 3333).
- The ID of a still is Track.Shot.Version, for example 1.3.1 (pp. 3334, 3337).
  - The numbers are the video track, the place of the clip in the timeline and the version of the grade.
- A Project Settings option gives each new still a label automatically (p. 3338).
  - The choices are the clip name, the version name, a timecode, the timeline name, the display LUT name and a custom text.
- Change Label, or a double-click under the ID, gives a still a label (p. 3338).
- A click, a Shift-click and a Command-click select stills (p. 3334).
- The menu of a still has Select All, Select Current to Last and Select First to Current (p. 3334).
- Delete Selected in the menu of a still deletes the selected stills. Only this menu deletes stills (p. 3334).
- Properties shows the date, the clip, and the source and record timecodes of a still (p. 3334).
- Each still is a DPX file in the folder of the project setting "Gallery stills location" (p. 3335).
- The options at the top right are Still Size, Sort Stills, Thumbnail View, List View, Search and Gallery View (p. 3339).
  - Sort Stills sorts by Still ID, Label, Record Timecode, Source Timecode, Grab Date or Create Time.
- Match Reference Wipe Frame moves the playhead to the frame of a still and selects its clip (p. 3339).
- The menu of the empty Gallery has these items (p. 3340):
  - Switch Wipe Mode, Trace Timeline, One Still Per Scene and Apply Display LUT.
  - Apply Grades Using, Use labels on still export, Show All Stills and Show Current Timeline Only.
- Live Preview shows the grade of a still on the current node while the pointer is on the still (p. 3335).
  - Hover Scrub Preview sets whether the thumbnail and the viewer scrub the clip (pp. 3335–3336).
- Measured in Resolve Studio 21.2 (`fixtures/color/NOTES.md` of the test data). Each item gives the input, the output and the number of cases. It is not a rule.
  - One still of each clip of three timelines [21]: the ID started with the video track and the place of the clip on it.
    - A grab on a covered clip gave a still of the clip on top, with the ID of that clip.
    - The DRX file held the first two numbers as `<Label>`.
  - A clip put before a clip of V2, on one timeline [2]: the older stills of the later clip kept 2.1. New stills of it showed 2.2.
  - In each case below, the third number was the place of the still among the stills with the same album, timeline and first two numbers.
    - The order was the record timecode, then the time of the grab [38 stills].
    - Grabs of two clips in turn, on two timelines [2]: 1.1.2, 1.4.2, 1.1.3, 1.4.3.
    - Two albums [1]: each album counted its own stills.
    - The delete of the middle one of three stills [1]: the last still then showed 1.1.2.
    - Four stills of one clip exported as 1.1.1 to 1.1.4. A fifth still showed 1.1.1, and the others 1.1.2 to 1.1.5 [1].
      - After the delete of the fifth still, the others showed 1.1.1 to 1.1.4.
    - Local versions [1]: a still on a second version showed 1.5.2. A still on version 1 after it showed 1.5.3.
    - p. 3337 says that the third number is the version of the grade. In these cases it was not.
  - An export of one album wrote one file name for each ID. Stills of four timelines with the same ID wrote over each other [1].
  - A still grabbed in the Timeline mode showed 1.3.1.T. A still grabbed in the Group Pre Clip mode showed 1.1.1.PRE. One case each.
  - The new stills of the runs had no label. The picture had the size of the timeline, at 1920x1080 and at 588x260 [2].
  - Six stills of three timelines had the size of the timeline: 1080x1920, 3840x2160 and 1920x1080, also from 1280x720 media [6].
  - Undo of Grab Still and of Delete Selected: one case each ([gotchas](../gotchas.md#resolves-edits)).
  - The menu of the empty Gallery also had the Copy Grade items and Copy Grade From Current Project Still.

### Albums and Memories (pp. 3341–3343, 3347–3348, 3364–3365, 3401)

- A new still goes into the open album. A new project has the album "Stills 1" (p. 3341).
- The Album button shows the list of albums (p. 3341).
- Add Still Album and Add PowerGrade Album add an album. A double-click renames an album (p. 3341).
- A drag of a still onto an album moves the still. Remove Current Album deletes the album and its stills, after a prompt (p. 3341).
- The stills of a PowerGrade album are in all projects of the project library. The last PowerGrade album cannot be deleted (p. 3342).
- The Timelines album shows the grade of each clip of a timeline of the project (p. 3343).
- Copy Active Layer, in the menu of a clip of the Timelines album, copies only the shown node stack layer (p. 3401).
- A memory holds a grade as a still does (p. 3347).
- Save Memory A to H is Option-1 to Option-8. Load Memory A to H is Command-1 to Command-8 (pp. 3348, 3365).
- A save into a full memory replaces its grade. Clear in the menu of a memory empties it (p. 3348).
- A drag copies a still to a memory, and a memory to the Gallery (p. 3348).
- Measured, one case each:
  - Add Still Album added "Stills 2" and opened it. A drag of a still onto that album moved the still.
  - The Memories menu showed no key for Load Memory and had no Clear. Ctrl+1 over the viewer opened the Media Pool, with a Finnish keyboard.
  - Alt+1 saved Memory A from a clip with L1 Gain 2 and L2 Lift 0.1, in the Timeline mode.
  - Load Memory A on another clip with L1 shown put Gain 2 into L1. With L2 shown, it put Lift 0.1 into L2.
  - The clip did not get the Contrast 1.2 of the timeline grade.

### Wipes (pp. 3084–3090, 3336–3337)

- These show a still against the current clip in the viewer (pp. 3085, 3336):
  - A double-click on the still.
  - The Image Wipe button.
  - Color > Stills > Play Still (Command-W).
- A drag in the viewer moves the wipe (p. 3085).
- The styles are Horizontal, Vertical, Diagonal, Mix, Alpha, Difference, Box, Venetian Blind and Checkerboard (p. 3085).
- Invert Wipe (Option-W) swaps the two sides (p. 3086).
- Next Still (Option-Command-N) and Previous Still (Option-Command-B) select another still (p. 3336).
- The Reference Wipe Mode is Gallery, Timeline or Offline. Wipe Timeline Clip wipes against another clip with no still (pp. 3086–3087).
- The Split Screen modes show up to 16 pictures (pp. 3088–3090).
  - Current Group, Selected Album, Selected Clips, Selected Still Grades and Selected Still Images are some of them.

### Apply Grade (pp. 3361–3373, 3405, 3464)

- A still and a memory hold the clip grade and the timeline grade (pp. 3362, 3405).
- A still holds all node stack layers of the clip (p. 3400).
- Apply Grade gives the selected clips the grade of a still, a memory or a clip (pp. 3364–3365).
  - The mode of the Node Editor selects the grade that changes: Clip or Timeline (pp. 3362, 3405).
  - The grade replaces the grade of each target clip (p. 3365).
- These apply a still: Apply Grade in its menu, a middle-click on it, or a drag of it into the viewer (pp. 3364–3366).
- Preserve number of nodes, in the menu of the empty Gallery, keeps 0 to 10 nodes of the target (p. 3361).
  - At 5, Apply Grade leaves out the first five nodes of the still, when the target has five nodes at least.
- Apply Grades Using sets the keyframes that a grade gives (pp. 3340, 3363):
  - No Keyframes: the grade at the frame of the still.
  - Keyframes Aligning Source Timecode: on the same source timecode. With no overlap, as the next item.
  - Keyframes Aligning Start Frames: at the same distance from the start of the clip.
- The manual also says that a still holds no keyframes by default. It then holds the keyframe before the playhead (p. 3464).
- Measured in Resolve Studio 21.2, one grade: a still of a clip with keyframes at 20 and 40 kept both ([spec §8.22](../spec.md#822-color-page-keyframes-pp-34533465)).
- Apply Grade from a clip puts the keyframes on the same source timecode (p. 3464).
- The All, Color and Sizing mode of the Keyframe Editor selects what a grade gives: the grade, the Input Sizing or both (p. 3363).
- Align Keyframes to Current Frame, in the Apply Grade submenu of the viewer, puts the first keyframe on the playhead (p. 3367).
  - Measured, one still with the grab frame 30 and keyframes at 20 and 40. Playhead 30 gave 20 and 40. Playhead 50 gave 40 and 60.
  - Thus the first keyframe did not go to the playhead. Both cases fit "the grab frame goes to the playhead". A still with another grab frame can break that.
- Append Node Graph adds the nodes of the still after the nodes of the clip (p. 3366).
  - A drop of a still onto a link adds the nodes there.
- Apply Grade From One Clip Prior (=) and Apply Grade From Two Clips Prior (-) copy the grade of an earlier clip (p. 3364).
- Command-C and Command-V on the Thumbnail timeline copy the whole grade. In the Node Editor, they copy one node (pp. 3361–3362).
- Preview Memory (Option-Shift-P) starts a preview of the next applied grade. A second Preview Memory gives the earlier grade back (p. 3365).
- Display Node Graph shows the nodes of a still or a clip in a window (pp. 3368–3372).
  - Its nodes drop onto the nodes or the links of the current grade.
  - Apply Color, Apply PTZR, Apply Source and Apply All copy parts of the grade.
- Ripple Node Changes to Selected Clips gives a node change to the node of the same number in the selected clips (p. 3372).
- Append Node to Selected Clips adds the current node at the end of the grade of each selected clip (p. 3373).
- Measured. Each item gives the input, the output and the number of cases. It is not a rule.
  - A still by its menu, with clips 02 (current) and 04 selected: both clips got the grade. A middle-click did the same. One case each.
  - A drag of that still into the viewer, with 02 and 04 selected, changed only 02. One case.
  - A still with L1 Gain 2 and L2 Lift 0.1, by Apply Grade: the target got both layers. One case.
  - The same still by Apply Active Layer: with L1 shown, only L1 got Gain 2. With L2 shown, only L2 got Lift 0.1. One case each.
  - In the Timeline mode, a still with no timeline grade emptied the timeline grade. One case.
  - In the Timeline mode, a still grabbed in that mode gave its timeline node back. The clip grade did not change in both cases.
  - Undo, one sequence on two clips: Ctrl+Z with 02 current took back the last apply on 02 only. A second Ctrl+Z took back an earlier apply on 02.
  - Then Ctrl+Z with 04 current took back the apply on 04.
  - No Keyframes was ticked in this project. It is not known if it is the default of a new project.
  - No Keyframes, one grade: Dynamic keyframes at 20 (Saturation 0.5) and at 40 (Saturation 0), and other values at the start.
    - Grabs at 25, 30, 35 and 39 gave the values of the keyframe at 20. Grabs at 40 and 50 gave those of 40. A grab at 10 gave the start [8].
    - Grabs at 0, 10, 20, 25, 30, 39, 40 and 50, each on four clips of four timelines [32]. They gave the same values as above.
    - In each of these cases, the target got the values of the last keyframe at or before the grab frame.
    - Each of these targets got one node and no keyframes.
    - The blend at 30 is Saturation 0.25. The apply gave 0.5. A window apply of the grab at 30 gave the same values.
    - p. 3340 says that No Keyframes gives the grade at the frame of the still. These applies of one grade did not.
  - Keyframes Aligning Start Frames, the same grade, with keyframes at frames 20 and 40 of a clip from frame 0 of its file [4]:
    - Four clips on three timelines got keyframes at their frames 20 and 40. Two clips of the same file started at file frames 10 and 300. Two clips were of other files.
    - The source clip started at frame 0 of its file. Thus these cases do not tell the start of the clip from the start of the file.
  - Keyframes Aligning Source Timecode, the same grade [5]:
    - Two clips of the same file from frames 10 and 30 got keyframes on file frames 20 and 40. A clip of another file at 25 fps did too.
    - A clip of the same file from frame 300 got keyframes at its frames 20 and 40, as Start Frames gives.
    - A clip of another file that starts after both keyframes got no keyframes, and the values of the keyframe at 40.
  - Preserve number of nodes 1, a source of 2 nodes and a target of 2 nodes: node 1 of the target stayed. Node 2 came from the still.
  - Preserve number of nodes 3, the same source and target: both target nodes stayed. One case each.
  - Append Node Graph of a 1-node still with keyframes onto a 2-node clip: the node of the still came after node 2. One case.
    - The keyframes showed at source frames 20 and 40, also with No Keyframes. The clip starts at source frame 86400, so this reading is not checked.
  - One Clip Prior on the fourth thumbnail gave the grade of the third, on V2. Two Clips Prior gave the second. One layout.
  - The menu shows the keys of these two items as Shift+= and Shift+-.
  - Ctrl+V of a node onto another node replaced all settings of the target node. One case.
  - Ripple Node Changes, two changes in one sequence:
    - Gamma 0 to 0.05 on the current clip set Gamma 0.05 on the selected clip. Its other values stayed.
    - Then Saturation 25 to 30 set the Saturation of the selected clip from 50 to 30, not to 55. Its Gamma stayed 0.05.

### Shared nodes (pp. 3373–3375)

- Save as Shared Node, in the menu of a corrector, makes a Shared node. A new Shared node is locked (p. 3374).
- A change to a Shared node goes to each copy of it in the grades of other clips (p. 3374).
- The Add Node submenu lists the Shared nodes by their labels (p. 3374).
- A copied grade and a still copy their Shared nodes (p. 3375).
- Convert to Corrector makes a Shared node a plain corrector again (p. 3375).
- Delete Shared Node keeps each copy, clears its effect and removes it from the Add Node submenu (p. 3375).

### Groups (pp. 3114–3115, 3373, 3377–3382, 3400–3401, 3405)

- A group is a set of clips with a Group Pre-Clip grade and a Group Post-Clip grade (p. 3379).
- Add Into New Group, in the menu of the selected thumbnails, makes a group with a name (p. 3377).
- The menu of a grouped clip has Group > name > Load, Assign to Group, Rename and Delete (pp. 3377–3378).
- Remove From Group takes the selected clips out of their group (p. 3378).
- Delete keeps the last grade in each clip (p. 3378).
- A grouped clip shows a green link badge on its thumbnail (p. 3377).
- The current group is the group of the last selected clip (p. 3377).
- The grades act in this order: Group Pre-Clip, Clip, Group Post-Clip and Timeline (pp. 3379, 3400).
- The chart puts the Pre-Clip grade before the effects of the Edit page (p. 3407; [spec §13](../spec.md#13-order-of-operations-pp-287296-3407)).
- With Post Group Node Stack Layer on, the last node stack layer acts after the Group Post-Clip grade (p. 3401).
- For a grouped clip, the Node Editor has four dots: Group Pre-Clip, Clip, Group Post-Clip and Timeline (pp. 3378–3379, 3405).
- Each mode of the Node Editor has its own undo stack (p. 3381).
- A still of a grouped clip shows all its grades. It holds only the grade of the shown mode (p. 3381).
- Apply Grade replaces the grade of the shown mode (p. 3381).
- Collapse Group Grades puts the Pre-Clip nodes before the clip nodes and the Post-Clip nodes after them (pp. 3381–3382).
  - The clip then leaves the group.
- Ripple Node Changes to Current Group gives a node change to the clips of the group (p. 3373).
- The timeline filter Grouped has a submenu with each group (p. 3115).
- Resolve makes a smart filter for the current group, under Grouped > Current Group (p. 3378).
- A group can have versions (p. 3382).
- Measured. Each item gives the input, the output and the number of cases. It is not a rule.
  - Inputs: a Pre-Clip LUT x², a clip offset of 0.1 and a Post-Clip LUT 0.8x + 0.1.
    - At 1920x1080 and at 588x260, the order Pre-Clip, clip, Post-Clip fit the render. The other five orders did not.
    - This is one set of inputs, with no timeline grade, no effect and one layer.
  - A clip of another timeline joined a group. One case.
  - The OTIO file of one timeline with a group held no group. One export.
  - The menu of the current thumbnail had Add into Current Group, Add into a New Group, Remove from Group and Groups. Seen once.
  - Groups listed each group, with Assign to Group, Load, Rename, Delete and Versions. Seen once.
  - Three grouped clips showed a link badge under their thumbnails.
  - A still grabbed in the Group Pre Clip mode held the Pre-Clip grade. Its picture was the full output. One case.
  - Apply Grade in the Group Post Clip mode replaced the Post-Clip grade. Three thumbnails changed. One case.
  - Collapse Group Grades on one clip put layer 1 in the order Pre-Clip, clip, Post-Clip. The clip left the group. One case.
  - Remove from Group on one clip kept its clip grade. Delete of a group showed a prompt with Cancel and Delete. One case each.
  - Ctrl+Z right after Remove from Group took back the last change of the clip grade. The clip stayed out of the group. One case.

### Import and export (pp. 3343–3347, 3383)

- Import, in the menu of the empty Gallery, reads image files. It also reads a DRX file of the same name (p. 3346).
- Export writes an image and a DRX file of each selected still. The file type DRX writes the DRX file alone (p. 3347).
- The image formats are DPX, CIN, TIFF, JPEG, PNG, PPM, BMP and XPM (p. 3346).
- The Gallery window (Workspace > Gallery) shows the stills, the timelines and the PowerGrades of other projects (pp. 3343–3345).

## What vidcut has

- A clip grade with node stack layers, and a timeline grade (`model::Grade`, [spec §8.21](../spec.md#821-color-page-nodes-pp-33853399-34093415)).
- `grade.copy` and `grade.paste` copy the settings of one corrector. A pasted keyframe keeps its distance from the start of the clip.
- `clip.paste_attributes` with `grade` gives clips the whole grade of a copied clip ([spec §8.26](../spec.md#826-paste-attributes-pp-11551156-measured-in-resolve-studio-212)).
- The Thumbnail timeline of the Color page (`thumbnail_timeline.rs`):
  - A click, a Ctrl-click and a Shift-click select clips, as measured on four timelines ([spec](../spec.md)). The Color and Deliver pages share the selection.
  - The measured cases do not settle these. Each has one case or none:
    - A click on the clip that holds the playhead: in one case, the playhead stayed. vidcut does the same.
    - A Ctrl+Shift-click: in one case, it acted as a click. vidcut does nothing on it.
    - A move of the playhead when the current clip has the red tint: not measured. vidcut keeps that clip selected.
    - A filter that hides a selected clip: not measured. vidcut keeps the clip selected.
    - In one Shift-click case of 30, the notes show no red tint on the current clip.
  - It has no grade items in its menu.
- The Gallery of the Color page ([spec §8.31](../spec.md#831-gallery-and-apply-grade-pp-3050-33323341-33613366-3400-3405)):
  - `gallery.grab` grabs a still: Color > Grab Still, Ctrl+Alt+G and the menu of the viewer.
  - A still holds the clip grade with all layers and keyframes, the timeline grade, the clip, the timeline and the frame.
  - Its picture is a 32-bit float OpenEXR file in the stills folder, beside the proxies.
  - The ID of a still is `s1`, `s2` and so on. A still has no label yet.
  - A click and a Ctrl-click select stills. `gallery.remove` and Delete Selected delete them.
  - `gallery.apply`, Apply Grade and a middle-click give the grade of a still, in the mode of the Node Editor.
    - In the Clip mode, the targets are the clips selected in the Thumbnail timeline and the clip at the playhead.
    - A still whose grade has keyframes does not apply.
  - `gallery.get` lists the stills. Each grab, delete and apply is one undo step.
- The clip filter greys Grouped (`ClipFilter`). Switch to Node Layer greys Group Pre Clip and Group Post Clip.
- The viewer has no wipe and no split screen.
- `Project.groups` holds the synced groups of the Media tab. Thus color groups need another name.
- The looks of titles and subtitles are files in a folder of the user (`looks::dir`).

## Build steps

The steps are in order of value. Each step is one pull request.

Each step has requests for the CLI, IPC and MCP. Each edit is one undo step, also where Resolve has no undo.

### Stills and Apply Grade

The Gallery panel, Grab Still, Delete Selected, a click and a Ctrl-click on stills, and Apply Grade of a still with no keyframes are built.

These items wait for measured cases. Each gives its reason:

- The ID of a still, as measured: the track and the place of the clip at the grab, then the place of the still.
  - Local versions did not change the ID in the measured cases. Thus vidcut needs no versions for it ([versions.md](versions.md)).
  - The Rules run tests a clip put before the first clip of a track, Change Label and a move to another album first.
- Change Label, or a double-click under the ID, gives a still a label. The request is `gallery.set`.
  - The Labels run tests the dialog and its undo.
- Apply Grades Using sets the keyframes that Apply Grade gives. With it, a still whose grade has keyframes applies.
  - The Rules run tests the three modes on more grades before this item.
  - No Keyframes gives the grade at the frame of the still, as p. 3340 says. gotchas.md records what Resolve does.
  - It removes the line of the keyframes of a still from [gaps.md](gaps.md).
- Apply Active Layer gives only the shown node stack layer.
  - Why it waits: Resolve did this for one still on one clip, once with each layer shown. The Rules run adds clips and timelines.
- A drag of a still into the viewer applies its grade.
  - Why it waits: in one case, with two clips selected, only the clip at the playhead changed. The Rules run adds cases.
- A Shift-click on a still selects a range of stills (p. 3334).
  - Why it waits: what the range holds is not measured. The Rules run measures it, as for the Thumbnail timeline.
- A click on a clip under a clip of a higher track makes it the current clip. Resolve did this in three cases.
  - The Rules run tests what playback and the arrow keys then do.
  - The Rules run also tests if the red tint of the clip at the playhead changes the targets of Apply Grade.
- Export of stills writes the picture formats of Resolve ([Import and export](#import-and-export)).
- Runs: the Labels run and the Rules run.

### Albums and Memories

- The album list: Add Still Album, rename, a drag of a still onto an album and Remove Current Album.
- Memories A to H: Save Memory (Alt+1 to Alt+8), Load Memory (Ctrl+1 to Ctrl+8), Clear and the drags.
- Album changes and memory changes are undo steps. gotchas.md records that Resolve has no undo for them.
- Runs: the Gallery menus run.

### Copy options

- Preserve number of nodes, and Append Node Graph from the menu and from a drop onto a link.
- Apply Grade From One Clip Prior (=) and Apply Grade From Two Clips Prior (-).
- Ctrl+C and Ctrl+V of the whole grade on the Thumbnail timeline.
- Preview Memory (Alt+Shift+P) and Align Keyframes to Current Frame.
- Runs: the copy run.

### Groups

- A color group has a name, a Pre-Clip grade and a Post-Clip grade. A clip holds the ID of its group.
- The render adds the Pre-Clip grade and the Post-Clip grade at the places of the chart ([spec §13](../spec.md#13-order-of-operations-pp-287296-3407)).
  - One set of inputs fit that order in Resolve. The Rules run and the Groups rest run test more.
- The Node Editor of a grouped clip gets the Group Pre-Clip and Group Post-Clip modes: the dots, the switch and Switch to Node Layer.
- The Thumbnail timeline gets the group items of its menu and the group badge.
- A still and Apply Grade use the mode of the Node Editor. Resolve did this in one case of each.
- Requests for groups, and `layer` or a group mode in each grade request.
- OTIO: vidcut metadata keeps the group of each clip and the grades of its group. The groups run tells what a Resolve OTIO file holds.
- vidcut keeps one undo history. Resolve has an undo for each clip and none for group changes. The owner decides before this step.
- The versions of a group are in [versions.md](versions.md).
- Runs: the groups run.

### Group tools

- Collapse Group Grades.
- The Grouped filter with each group, and Current Group. It removes the groups line from [gaps.md](gaps.md).
- Post Group Node Stack Layer in the Project Settings. It removes the Post Group Node Stack Layer line from [gaps.md](gaps.md).
- Ripple Node Changes to Current Group.
- Runs: the groups run.

### Wipes

- Image Wipe, Play Still (Ctrl+W), Next Still and Previous Still.
- The wipe styles, Invert Wipe (Alt+W) and a drag in the viewer.
- Match Reference Wipe Frame.
- Runs: the wipes run.

### Timelines album and Display Node Graph

- The Timelines album, with each timeline of the project.
- Copy Active Layer. It removes Copy Active Layer from [gaps.md](gaps.md).
- The Display Node Graph window and its Apply buttons.
- Runs: the Timelines album run.

### Shared nodes

- Save as Shared Node, the Add Node entries, Convert to Corrector and Delete Shared Node.
- Runs: the shared nodes run.

### Ripple and append to selected clips

- Ripple Node Changes to Selected Clips and Append Node to Selected Clips.
- Runs: the copy run.

### Import and export

- Import and Export of stills read and write DRX files and the picture formats of Resolve. Thus grades go both ways.
- vidcut has no grade file of its own.
- The DRX file is Zstandard and protobuf. `fixtures/color/NOTES.md` of the test data has the decoded parts.
- Runs: the later run.

### PowerGrade albums

- PowerGrade albums in a folder of the user, as the looks are. Thus all projects on the computer share them.
- Runs: the later run.

### Gallery tools

- Automatic labels, Sort Stills, List View, Search, Still Size and Properties.
  - Properties and Sort Stills need the timecodes and the time of the grab in each still.
- Grab All Stills, Grab Missing Stills, Trace Timeline, One Still Per Scene and Show Current Timeline Only.
- Runs: the later run.

### Live Preview and split screens

- Live Preview, Hover Scrub Preview, and the split screens of the Gallery and of groups.
- Runs: the later run.

### Gallery window

- The stills, the timelines and the PowerGrades of other project files.
- Runs: the later run.

## Resolve runs

Each run is in the project "vidcut measure". [Script] steps use the scripting API. [Window] steps need the windows of Resolve.

A menu item, a key or a default value needs one check. Each other behavior needs several cases on different clips.
These cases use at least three timelines that differ in size, frame rate, media and tracks.

`fixtures/color/NOTES.md` of the test data has the measured results.

- **Groups rest** ([script] and [window], 15 min): the place of the Pre-Clip grade against an effect of the Edit page, in a render.
  - Also Post Group Node Stack Layer, Ripple Node Changes to Current Group and the Grouped filter.
- **Labels** ([window], 5 min): the Change Label dialog and its undo.
- **Rules run** ([script] and [window], about 4 h in two or three parts). It adds cases for each other behavior above that rests on one or two cases.
  - Current clip: playback, arrow keys and three tracks after a click on a covered clip.
  - Thumbnail selection: a click in the clip at the playhead, a Ctrl+Shift-click, a move of the playhead with the red tint, and a filter.
  - The default of Apply Grades Using in a new project. This is a default value, so one check is enough.
  - Still IDs: a clip put before the first clip of a track on a second timeline, Change Label and a move to another album.
  - No Keyframes: Static keys, two nodes with other keys, window and sizing keys, and an eased key.
  - The other keyframe modes: a source clip from a later frame, a speed change, and more clips of other files with no overlap.
  - Align Keyframes: stills grabbed at other frames, and keys that go before the start of the clip or after its end.
  - Preserve number of nodes: 1 to 3 and 10 against targets of 1 to 4 nodes, parallel nodes, two layers and keyframes.
  - Append Node Graph: a target on another source range, parallel nodes, and each keyframe mode.
  - Clip prior: three tracks with ties, the first clip, a filter and a disabled track.
  - Ripple Node Changes: two values, a value set back, a missing node, a keyframed value, L2, curves and windows.
  - Undo: three clips, the group modes, the group items, a ripple, a memory load, the albums, Grab Still and Delete Selected.
  - Groups: a timeline grade, PRE stills in other modes, Collapse with two layers and two groups.
  - Memories and layers: a save from each layer, a load into another layer, the Timeline mode and a full memory.
  - Stills: Apply Active Layer, a drag into the viewer and a Shift-click, each on three timelines.
- **Wipes run** ([window], 15 min): the wipe styles, the drag, Invert Wipe and the picture of the still side.
- **Timelines album run** ([window], 15 min): the Timelines album, Copy Active Layer and Display Node Graph.
- **Shared nodes run** ([window], 15 min): the Shared node items and what a change of a Shared node does.
- **Later run** ([window], 20 min): import, export, sorts, PowerGrade albums and Live Preview.
