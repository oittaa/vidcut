# The Gallery, stills, groups and Apply Grade

**When:** now. The owner picked this work on 2026-11-26.

The Gallery of the Color page keeps stills of graded frames. A still holds a picture and a grade. Apply Grade gives a grade to other clips.

A group gives a set of clips two shared grades: one before their clip grades and one after them.

vidcut has none of these. This file tells what Resolve does, what vidcut has, the build steps in order of value, and the Resolve runs of each step.

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
  - The ID of a still of the third clip of V1 started with 1.3. The ID of a still of the first clip of V2 started with 2.1. One case each.
  - Four stills of one clip, all of grade version 1, in one album, exported as 1.1.1 to 1.1.4.
  - After one more grab of that clip, the new still showed 1.1.1 and the others 1.1.2 to 1.1.5. After its delete, they showed 1.1.1 to 1.1.4.
  - A still grabbed in the Timeline mode showed 1.3.1.T. A still grabbed in the Group Pre Clip mode showed 1.1.1.PRE. One case each.
  - The new stills of the run had no label. The picture had the size of the timeline, at 1920x1080 and at 588x260.
  - Ctrl+Z after one grab kept the still. Delete Selected of one still asked nothing, and Ctrl+Z did not bring it back. One case each.
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
    - Grabs at 25, 30, 35 and 39 gave the values of the keyframe at 20. Grabs at 40 and 50 gave those of 40. A grab at 10 gave the start.
    - The blend at 30 is Saturation 0.25. The apply gave 0.5. A window apply of the grab at 30 gave the same values.
    - p. 3340 says that No Keyframes gives the grade at the frame of the still. These eight applies of one grade did not.
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
  - A click, a Ctrl-click and a Shift-click select clips ([spec](../spec.md)). The Color and Deliver pages share the selection.
  - These rules come from one sequence of Resolve clicks. The Selection run tests them.
  - It has no grade items in its menu.
- The clip filter greys Grouped (`ClipFilter`). Switch to Node Layer greys Group Pre Clip and Group Post Clip.
- The viewer has no wipe and no split screen.
- `Project.groups` holds the synced groups of the Media tab. Thus color groups need another name.
- The looks of titles and subtitles are files in a folder of the user (`looks::dir`).

## Build steps

The steps are in order of value. Each step is one pull request.

Each step has requests for the CLI, IPC and MCP. Each edit is one undo step, also where Resolve has no undo.

### Stills and Apply Grade

- A Gallery panel at the left of the viewer, with the stills of one album.
- Grab Still (Ctrl+Alt+G, and the menu of the viewer) saves a still of the frame at the playhead.
  - The still holds all node stack layers of the clip grade, the timeline grade and their keyframes.
  - It also holds its ID, the clip, the timeline, the frame, the timecodes and the time of the grab.
  - The version in the ID is 1, because vidcut has no versions ([versions.md](versions.md)).
- Apply Grade gives the selected clips the grade of a still: its menu, a middle-click, or a drag into the viewer.
  - It changes the grade of the mode of the Node Editor.
  - Apply Grades Using sets the keyframes. The Rules run tests the three modes on more grades before this step.
  - No Keyframes gives the grade at the frame of the still, as p. 3340 says. gotchas.md records what Resolve does.
- In one Resolve case, a click on a clip under a clip of a higher track made it the current clip. The Rules run tests more cases first.
- Grab Still and Delete Selected are undo steps. gotchas.md records that Resolve has no undo for them.
- Delete Selected, Change Label and the selection of stills.
- Requests: `gallery.grab`, `gallery.apply`, `gallery.set`, `gallery.remove` and `gallery.get`.
- The project holds the stills. Each picture is a 32-bit float OpenEXR file in a folder next to the project file.
  - This file is the storage of vidcut, as the project file is. Thus it keeps the values of the picture exactly.
  - Export of stills writes the picture formats of Resolve ([Import and export](#import-and-export)).
- It removes two lines from [gaps.md](gaps.md): the keyframes of a still, and the node stack layers of a still.
- Runs: the stills run and the Gallery menus run.

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
- **Selection run** ([window], 100 min): the selection of the Thumbnail timeline on four timelines of 4, 5, 7 and 9 clips.
  - Clicks: a click inside a selection, and a Ctrl-click on the last clip and on the current clip.
  - Shift-clicks: forward, backward, twice in a row, after a Ctrl-click that removes, across tracks, and with the current clip outside the range.
  - Pages: Edit to Color and back with other Edit selections and playheads, Color and Deliver in several orders, and a timeline switch.
- **Rules run** ([script] and [window], about 4 h in two or three parts). It adds cases for each other behavior above that rests on one or two cases.
  - Current clip: playback, arrow keys and three tracks after a click on a covered clip.
  - The default of Apply Grades Using in a new project. This is a default value, so one check is enough.
  - Still IDs: ties, a clip put in before, two clips in turn, two albums, a delete in the middle and grade version 2.
  - No Keyframes: Static keys, a grab on a key, two nodes with other keys, window and sizing keys, and an eased key.
  - The other keyframe modes: a target that covers one key, a speed change, and a target shorter than the keys.
  - Align Keyframes: stills grabbed at other frames, and keys that go before the start of the clip or after its end.
  - Preserve number of nodes: 1 to 3 and 10 against targets of 1 to 4 nodes, parallel nodes, two layers and keyframes.
  - Append Node Graph: a target on another source range, parallel nodes, and each keyframe mode.
  - Clip prior: three tracks with ties, the first clip, a filter and a disabled track.
  - Ripple Node Changes: two values, a value set back, a missing node, a keyframed value, L2, curves and windows.
  - Undo: three clips, the group modes, the group items, a ripple, a memory load and the albums.
  - Groups: a timeline grade, PRE stills in other modes, Collapse with two layers and two groups.
  - Memories and layers: a save from each layer, a load into another layer, the Timeline mode and a full memory.
- **Wipes run** ([window], 15 min): the wipe styles, the drag, Invert Wipe and the picture of the still side.
- **Timelines album run** ([window], 15 min): the Timelines album, Copy Active Layer and Display Node Graph.
- **Shared nodes run** ([window], 15 min): the Shared node items and what a change of a Shared node does.
- **Later run** ([window], 20 min): import, export, sorts, PowerGrade albums and Live Preview.
