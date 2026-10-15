# Cut page behavioral spec

vidcut copies the behavior of DaVinci Resolve's **Cut page** by default. This document describes that behavior in our own words. Each item carries a page reference to the *DaVinci Resolve 21.1 Reference Manual* (Chapters 33–39, pp. 635–790). Printed page numbers equal PDF page numbers. Deviations are listed in [deviations.md](deviations.md); the Decisions section at the end settles points the manual leaves open.

**Reading the manual:** it is 183 MB, so extract text with `pdftotext -f 635 -l 790 <manual>.pdf -`. The developer's copy lives in `$VIDCUT_TEST_DATA` (default `~/Downloads/vidcut-tmp`). Don't copy manual text into the repo; write behavior in your own words with page numbers.

**Key names:** the manual uses Mac keys. On Linux and Windows, Command → Ctrl and Option → Alt. On macOS the keys are the manual's.

**Markers:** **(later)** is wanted but not built, **(out)** is out of scope.

## 1. Scope

**In scope:**
- media pool, viewer, dual timeline
- editing and trimming, constant speed changes (D48)
- dissolves: cross dissolves, fades to and from black, audio cross fades (§8.11)
- clip/track audio levels and waveforms
- Quick Export, plus our Export dialog

**Out of scope (out):**
- effects, other transitions, titles, generators, keyframes
- color, Fusion, Smart Reframe, Close Up
- Sync Bin, Multi Source, Source Overwrite, Live Overwrite
- Replay, voiceover, subtitles
- AI tools: transcription, voice/dialogue tools
- Mixer FX, Paste Attributes, guides
- interchange formats: DRT, AAF, XML

## 2. Layout (pp. 636–637)

- **Main areas:** Media Pool on the left, a single Viewer on the right, and the timeline area across the bottom. The pool/viewer seam and the timeline height can be resized.
- **Top bar:** Media Pool tab, Mixer tab, Quick Export, Full Screen viewer. (Sync Bin, Transitions, Titles and Effects tabs and the Inspector: out.)
- **Below the Media Pool:** the edit-function buttons (§7), and Cut and Dissolve (§8.11).
- **Viewer chrome:**
  - mode buttons in the top-left
  - Duration field in the top-right
  - transport bar along the bottom: Previous Edit, Stop, Play, Next Edit, Loop, Mark In, Mark Out, timecode
  - audio meter to the right, with a monitoring mute button (p. 642)
- **Timeline area:**
  - **Menus at the top-left:**
    - Timeline Options (p. 645): Ripple On, Snap, Fixed Playhead, Display Clip Names, Display Clip Durations, Display Clip Status, Trim with Safe Edit, Trim to Audio, Boring Detector.
    - Timeline Actions (p. 645): Add Track, Split Clips, Add Marker, Join Match Edit Clips.
    - Edit Actions (p. 646): Trim Start to Playhead, Trim End to Playhead.
  - **Buttons:** Ripple On, Split, Marker.
  - **Upper timeline:** track-number buttons to its left select the target track; Audio Only and Video Only toggles sit there too (pp. 643, 694).

## 3. Media Pool (Ch. 34, pp. 653–675)

- **Import** (p. 654):
  - Ctrl+I, the Import Media button, or right-click > Import Media opens a file dialog with multi-select. It starts in the folder media were last imported from, remembered across runs, else in the project's folder. Open Project and New Project start where a project was last opened or created, and exports where the last one went, the same way.
  - Import Media Folder imports a whole folder.
  - The first import into an empty project offers to change the project frame rate to match the media.
- **Stills and image sequences** (pp. 114, 366, 1194):
  - An image file (PNG, JPEG, TIFF, EXR, DPX, …) comes in as a still. Edited without a range or marks it lasts the standard still duration; its clips can be made up to 17 h 40 min long (D38).
  - Numbered image files (renders, scans) can come in as one clip, an image sequence, at a frame rate (D39).
- **Remove** (p. 654): Delete or Backspace, after a confirmation. Only the pool entry goes; files on disk stay.
- **Views:**
  - **Thumbnail** (p. 664):
    - hovering scrubs the clip
    - I/O while hovering marks source In/Out
    - clicking the thumbnail's lower-right corner shows its metadata
  - **List** (p. 665): sortable columns.
  - **Filmstrip** (p. 664): each clip is a row of its pictures as wide as the pool, under its name. Hovering scrubs it, and I/O mark the clip there, as on a thumbnail.
  - Hovering shows the frame under the pointer in both views. A sound file shows its waveform.
  - **Metadata** cards (later).
- **Sort and search** (pp. 666–667):
  - Sort Media By: Clip Name, Date Imported, Date Modified, Duration (ours), Timecode, ascending or descending. vidcut sorts by date added (the default), name, duration or file date.
  - The sort order is also the Source Tape order.
  - The search field filters live. It looks through the open bin and the bins inside it.
- **Bins** (pp. 655–656, 377):
  - The Master bin is the top level. The pool shows the open bin's own clips and the bins inside it as folders. A path at the top (Master › Interviews › Day 1) opens any level, and the Bins menu lists every bin as a hierarchy.
  - Double-click a bin to open it. New Bin is on the File menu (Shift+Ctrl+N), the Bins menu and the pool's right-click menu. Right-clicking a bin offers Rename Bin and Remove Bin.
  - Drag clips onto a bin (or onto a level of the path) to move them into it. A clip's menu offers Move to Bin and Create Bin With Selected Clips ("Bin N", p. 656).
  - Imports go into the open bin (p. 377).
  - Source Tape strings together the open bin's clips and its bins' clips, in the sort order (pp. 680, 696).
  - Removing a bin keeps its clips (deviation X6). Dragging a bin onto another bin (a folder, or a level of the path) moves it and everything inside it with `bins.rename`, keeping its name. Dropping it on itself, on a bin inside it, or on its current parent does nothing.
- **Clip Color** (pp. 667–668): right-click clips in the pool or the timeline and pick one of 16 colors, or Clear Color. A pool item shows its color as a dot, or as a patch in the List view. A timeline clip is tinted with its own color, or else its pool item's.
- **Relinking media** (pp. 668–669): an item whose file is missing is marked MISSING (red in the list), its clips are red on the timeline (pp. 509, 530), and the viewer shows Media Offline with the file's name where it would show them. The Relink Media button, shown while files are missing, finds them in a folder chosen (subfolders too); a clip's menu offers Relink… for one file. vidcut checks each file it finds (deviation X1).
- **Interaction:**
  - **Click:** selects. In Source Clip mode the Viewer shows the clip; in Source Tape mode the playhead jumps to the clip's first frame (pp. 680, 704).
  - **Double-click:** opens the clip in the Viewer as Source Clip (p. 680).
  - **Drag onto a timeline:** a drag-and-drop edit (§7.3).
- **Selection** (p. 719): click, Shift-click range, Ctrl-click toggle, marquee, Ctrl+A.

## 4. Viewer (pp. 639–640, 679–693, 713)

### 4.1 Modes
- **Source Clip** (p. 680):
  - shows the selected pool clip
  - a scroll area under the picture represents the whole clip, with the playhead, a zoomed-in waveform and draggable In/Out handles
  - Jog In and Jog Out controls at the ends nudge the marks by single frames
- **Source Tape** (pp. 680–681, 704–706):
  - all pool clips play end to end as one continuous tape, in pool sort order
  - selection is synced both ways: the clip under the playhead becomes selected in the pool, and clicking a pool clip moves the playhead to it
- **Timeline** (p. 681):
  - shows the timeline at the timeline playhead; there is no scroll area
  - icons mark the first and last frame of a clip
- **Switching:**
  - Q toggles between Timeline and the last-used source mode (p. 696).
  - Double-clicking a pool clip switches to Source Clip.
  - Any click or drag in the timeline area switches to Timeline (p. 681).

### 4.2 Transport (pp. 640, 687–688, 870–872)
- **Play and step:**
  - Space: play/stop.
  - J/K/L: shuttle. Repeated J/L speeds up; K+J or K+L plays slowly; holding K and tapping J/L steps a frame (pp. 696, 871).
  - ←/→ step one frame; Shift+←/→ step one second (Edit-page keys, p. 870).
- **Jumping:**
  - Up/Down: previous/next edit point (p. 640).
  - Home/End: start/end; Shift+I / Shift+O: go to In / Out (p. 870).
- **Loop:** Ctrl+/ (later).
- **Jog control:** click and drag to scrub finely (later).

### 4.3 Timecode (pp. 640, 688, 713–714)
- **Go to a time:**
  - `=` then a timecode goes to that absolute time.
  - `+` or `-` then a value jumps relative to the current position.
  - Clicking the timecode field allows typing.
  - Digits fill a timecode from the right: `100` is 1 second, `10000` is 1 minute. A value with a unit (`5s`, `450f`) is that duration.
- **Display options** (right-click):
  - source timecode or source frame count (source modes)
  - record timecode or record frame (Timeline mode)
  - drop-frame at 29.97 and 59.94, where the project counts so (decision D47)
  - Timer 1/2 and KeyKode (later)

### 4.4 In/Out (pp. 691–693, 700)
- **Source In/Out:**
  - I/O in a source mode, or while hovering a pool thumbnail
  - or by dragging the scroll-area handles
- **Timeline In/Out:** I/O in Timeline mode, or while dragging either timeline's ruler. The range is drawn on both rulers and can be dragged there.
- **Clearing:** Alt+X clears both marks.
- **Duration field (top right):** shows the marked source duration. Editing it moves the Out mark; it accepts frames, ±frames or a timecode (p. 693).

### 4.5 Other viewer features
- Full Screen (Shift+F) and Enhanced viewer (Alt+F) (later).
- Safe areas (later); Fast Review (later).
- Skimming with C held (later).
- Audio scrubbing (Shift+S; on by default, p. 876): dragging the playhead, and frame steps, play the sound under it (decision D42). Shift+S, Timeline > Audio Scrubbing and `player.audio_scrubbing` turn it off and on.

## 5. Dual timeline (pp. 66, 643–646, 689–692, 716–718)

**Upper timeline:**
- Always shows the whole program at full width; it has no zoom.
- Its playhead moves freely, and dragging its ruler scrubs the whole program. A sideways two-finger swipe on a trackpad moves the playhead too.
- Most edits work there too, and clips can be dragged between the two timelines.
- It shows the timeline In/Out range and markers.

**Lower timeline:**
- A detail view around the playhead (p. 644). Ctrl+wheel (deviation X21), a pinch on a trackpad, and − and + zoom it. Resolve Studio 21.1.1 zooms it too, though the manual calls its zoom fixed.
- A sideways two-finger swipe on a trackpad scrolls it (p. 66).
- **Fixed Playhead on (default):** the playhead stays at the center and the clips move under it. Dragging the ruler or a swipe moves the clips. Clips don't snap to the fixed playhead (p. 717).
- **Fixed Playhead off (Timeline Options):**
  - the clips stay put and the playhead moves
  - the view pages when the playhead reaches an edge, and a swipe scrolls it
  - clicking the ruler moves the playhead, and the playhead can be dragged

**Track display:**
- Combined video+audio tracks are numbered 1, 2, …; audio-only tracks A1, A2, … sit below them.
- A video clip shows a filmstrip. Its audio shows as a waveform band along the bottom, which is enlarged when the track is enlarged (p. 647). Each picture of the filmstrip is the frame under it, the first one the clip's first frame.
- Display toggles: clip names; durations (seconds:frames, bottom-left); status icons.
- **States:**
  - locked track: clips are stippled
  - disabled track: clips are dimmed
  - disabled clip: dull
  - muted clip: a mute icon

**Track headers:**
- Enlarge (one track at a time), Lock, Mute (audio), Video Enable (pp. 647, 678, 717): icons, enlarge on the left of the name, the others in columns at the right (D43).

**Automatic tools:**
- **Boring Detector** (Timeline Options, p. 690): highlights clips longer than "Boring Clips" (light gray) and shorter than "Jump Cuts" (red) on the upper timeline, live as you edit. Analyze Timeline Edits sets both (D31).
- **Detect Scene Cuts** (Timeline Actions, p. 690): splits the selected clips where their shots change (D30). Resolve's is Studio-only; vidcut has it in every build.

## 6. Track model (pp. 647, 649, 716, 721, 727)

- **Track 1 is the main story (A-roll):**
  - insert, delete, move and trim on Track 1 ripple, so it never has gaps
  - with **Ripple On** turned off it behaves like the other tracks
  - holding Alt while trimming turns rippling off temporarily
- **Tracks 2+ are B-roll:**
  - they never ripple themselves, and clips sit freely with gaps
  - each higher-track clip is attached to the Track 1 clip under its In point and follows that clip when Track 1 ripples (p. 649; D21)
  - trimming the anchor's Out to before the attached clip's In deletes the attached clip (p. 727)
  - higher tracks only ripple where there is no Track 1 video under them (p. 721; D22)
- **Audio tracks:** A1, A2, … sit below the video tracks and hold sound alone, such as music. Media without video go there by default (D26).
- **Track header** (p. 647): the target-track toggle (D3), lock, mute and video on/off. The target track's header and row are lighter, the header with an accent edge. Right-click adds a video or audio track or removes one (D27).
- **Overlapping video:** the highest enabled track is visible (p. 647). A disabled clip or a track with its video off shows what is under it.
- **Sound:** every audible track plays, mixed; mute and solo (§9) decide which are audible.
- **Linking:** video and audio of a clip are one combined clip. Trims move both, except that a J/L cut moves only the audio edit (§8.7). There is no link/unlink on the Cut page.

## 7. Edit functions (pp. 696–701)

**Workflow** (p. 696):
1. Pick a source.
2. Mark source In/Out.
3. Optionally pick the target track by clicking its header or number.
4. Press an edit button or key.
5. Press Q to review the result and Q again to go back.

Video Only and Audio Only limit what an edit takes (p. 694).

### 7.1 Smart Indicator (p. 696)
- **What it marks:** a marker on the ruler shows the edit point nearest the playhead. Some functions use that point instead of the exact playhead frame.
- **How it looks:** after idling it bobs, and the chosen edit flashes green. It flashes red if the clip there has no media left beyond its edge. vidcut shows the colors without the motion (deviation X15).

### 7.2 Edit functions
- **Smart Insert** (F9 in the Edit page; p. 697):
  - inserts at the Smart Indicator's edit point on the target track, never at an arbitrary frame
  - on Track 1, everything after that point moves right by the incoming duration
- **Append** (Shift+F12; p. 697): ignores the playhead and places the clip after the last clip.
- **Ripple Overwrite** (Shift+F10; p. 698):
  - with no timeline marks, it replaces the whole clip under the playhead
  - with a timeline In/Out, it replaces exactly that range, possibly across several clips
  - everything after shifts by (incoming − replaced) duration, in either direction
  - the source duration always wins; nothing is fitted to the gap
- **Overwrite** (F10; p. 701):
  - writes over the target track at the playhead, at the timeline In, or backtimed to a lone timeline Out
  - other clips don't move
- **Place On Top** (F12; pp. 699–700):
  - goes to the first track above every clip at that position, creating one if needed (D24)
  - ignores the selected track and never ripples
  - **Alignment priority:**
    1. A timeline In lines up with the source In.
    2. A lone timeline Out lines up with the source Out (backtimed).
    3. Within 5 frames of an edit with no marks, it lines up with that edit.
    4. Otherwise it lines up with the playhead.
- **Close Up, Source Overwrite (out).**

### 7.3 Drag and drop from the pool or Viewer (pp. 648, 679, 694–695)
- **Onto an empty timeline, or left of the first clip:** becomes the first clip.
- **Past either end of all clips:** appended or prepended.
- **Onto a clip, released right away** (the whole clip highlights): Ripple Overwrite replaces that clip.
- **Onto a clip after hovering briefly** (the highlight narrows to the incoming footprint): a plain Overwrite with no ripple, which may split the target.
- **Onto an edit point:** insert at that edit.
- **Into the gray area above the top track:** creates a new video+audio track. Below the bottom track, an audio clip creates an audio-only track.
- **Onto another track:** an overwrite there, never a ripple (only Track 1 pushes; D23). Sound alone dropped on a video track goes to the first audio track instead.

## 8. Timeline editing (Ch. 36, pp. 715–732)

### 8.1 Selection (pp. 719–720)
- **Clips:**
  - click selects, Ctrl+click toggles, Shift+click selects a range
  - a marquee from empty space selects what it touches
  - clicking empty space, or Ctrl+Shift+A, deselects
- **Alt+Shift+V:** selects every clip under the playhead.
- **Edit points:**
  - the pointer at the center of an edit selects both sides (a roll)
  - just left or right of center selects one side (a resize)
  - Ctrl+click adds edits
  - V selects the edit nearest the playhead; U cycles outgoing / incoming / both

### 8.2 Moving clips (pp. 720–723)
- **Onto another Track 1 clip, released quickly:** replaces it and ripples.
- **After hovering first:** overwrites the middle of the target, splitting it; the gap left behind closes on Track 1.
- **Over a neighbor's edge, the pointer not reaching it** (p. 722): the overlapping part of the neighbor is overwritten and the gap left behind closes, so the neighbor ends up shorter.
- **Onto an edit point** (it highlights): insert-at-edit. The gap left behind closes, and later clips (with their attached higher-track clips) shift. Works with several clips.
- **Track 1 clip dropped onto another track:** can only overwrite (p. 721). Track 1 closes up behind it.
- **Higher-track clips:** move freely and leave gaps. Moves between tracks: D25.

### 8.3 Copy, cut, paste (p. 723)
- Ctrl+C copies.
- Ctrl+X cuts and ripples the gap closed.
- Ctrl+V pastes at the playhead on the original track, overwriting.

### 8.4 Delete (pp. 721, 724–725)
- **Delete:** removes the selected clips. On Track 1 with Ripple On it ripples, and attached clips follow.
- **With a timeline In/Out set:** Delete removes that range.
- **Disable:** D disables or enables a clip's video. Right-click > Mute silences its audio.

### 8.5 Split (pp. 646, 723, 950)
- Ctrl+\, the Split button, or Split Clips splits at the playhead.
- Join Match Edit Clips (Timeline Actions) joins the parts of a split clip back into one (D49).

### 8.6 Trimming (pp. 693, 716, 718, 726–732)
- **The pointer position picks the operation:**
  - **Clip edge: resize.** A tooltip shows the frame delta and new duration. On Track 1 with Ripple On it ripples; elsewhere only that clip changes.
  - **Edit center: roll.** The total duration stays the same, and a white overlay shows the available handle media.
  - **Slip handle at the clip center: slip.** Several selected clips slip together.
  - **Lower audio band at an edit: audio-only roll**, i.e. a J/L cut (§8.7).
- **Safe Edit** (option, p. 732): a trim stops at the neighboring clip, with a small timer on the cursor; keep dragging to overwrite anyway.
- **Trim to Audio** (option): the waveform expands while trimming (later).
- **Trim Editor** (double-click an edit; pp. 729–730):
  - the Viewer shows the outgoing filmstrip on top and the incoming one below
  - the top handle trims the outgoing Out, the bottom handle the incoming In, and the middle handle rolls
  - ±1-frame buttons
  - vidcut shows the outgoing clip's last frame beside the incoming clip's first frame with the ±1 buttons; the draggable filmstrips are (later).
  - Esc or Close closes it. `gui.trim_editor` opens it, trims with it and closes it from scripts.
- **Trim Start/End to Playhead** (Edit Actions, p. 729): moves a clip's head or tail to the playhead.
- **Change Clip Speed** (R†, D48): a speed in percent, reversed, or a freeze frame, for every selected clip.
- **Change Clip Duration** (Ctrl+D, p. 693):
  - enter frames or a timecode, or pick 1/5/15 seconds
  - End extends to the end of the media
  - "Extend Beyond Clip Length" pads with black (later)
  - applies to every selected clip
- **Dynamic Trim** (W, pp. 730–732): W toggles the mode and the playhead turns yellow. J/K/L then trim the selected edit (Out, In or both by U), or slip or slide the selected clips (right-click the Dynamic Trim button to choose), instead of playing. With nothing selected it takes the edit nearest the playhead. Space plays around the selection, and Up/Down move to another edit; both apply what is pending first. Details: D32 and deviation X11.
- **Slide:** moves a clip keeping its media; the clip before it gets longer and the one after it shorter (Dynamic Trim, p. 731; `edit.slide`).

### 8.7 J/L cuts (p. 728)
Hover the lower audio part of a clip at an edit until the cursor shows a note, then drag. Only the audio edit point moves, as an audio roll. A click selects that audio edit, and `,` / `.` move it (D20).

### 8.8 Snapping (pp. 716–717)
- N toggles snapping. Clip edges, markers, the playhead and the timeline In/Out snap to each other, and a white line shows the alignment.
- Pressing N mid-drag is temporary; pressing it between drags sticks.

### 8.9 Markers (pp. 718–719)
- M adds a marker at the playhead, or edits the marker under it.
- Shift+Up/Down jumps between markers.
- Markers can be dragged.
- Alt+M deletes the marker at the playhead; Delete removes selected markers.

### 8.10 Undo (pp. 651–652)
- Ctrl+Z / Ctrl+Shift+Z, unlimited.
- A History list shows recent steps and can jump back to one.
- A new change discards the redo steps.

### 8.11 Dissolves (pp. 645, 747, 758–761, 1114, 1159, 1173)
- **Dissolve and Ctrl+T** (p. 758): a one-second Cross Dissolve on the edit nearest the playhead, centered on it. Ctrl+T puts it on the selected edit (V) instead, and U picks where: ending on the edit (the outgoing side selected), starting on it (the incoming side) or centered (both). Where the clips' sounds meet at that edit, a Cross Fade goes on the sound too (p. 1114). Details: D50.
- **Cut** (p. 759): the edit nearest the playhead becomes a cut again.
- **Timeline Actions** (p. 645): Add Dissolve to All Edits, Clear Transition from All Edits.
- **Handles** (pp. 760, 1159): through a dissolve each clip shows its media past its edge, so a dissolve is as long as the clips have frames there.
- **How it plays** (pp. 747, 1173): the outgoing picture fades out as the incoming one fades in, in a straight line. At an edge with a gap, the clip fades from or to black. A Cross Fade turns one sound down while the next comes up, at +3, 0 or −3 dB at its middle.
- **Out:** other transitions (wipes, Smooth Cut, the Transitions browser), the Transition Inspector's settings (style, ease, ratios, curves), dragging a transition's edges or moving it, Set Default Transition.

## 9. Audio (pp. 642, 649–650, 743, 771, 773)

- **Meter:** green is safe, yellow is near the limit, red risks clipping.
- **Mixer** (pp. 649–650): the Mixer button opens a channel strip per track with sound: name, meter, fader (dB) and mute/solo, plus the mix's meter. A dB scale stands beside each fader, its numbers thinned out when the fader is short, with 0 dB in the accent color and a line across the fader. Solo silences the tracks that aren't soloed, and mute/solo apply to export too (p. 650). A fader is heard as it is dragged (`mixer.preview`, preview.md §7) and saved when it is let go, as one `track.set`. Track pan and FX are out of scope.
- **Viewer meter and monitoring mute** (p. 642): a meter beside the viewer shows what it plays. Its speaker button mutes playback only, not the export.
- **Clip volume:** one gain value per clip, and clip mute.
- **Ripple Delete Silence** (p. 773): select clips, then Clip > Audio Operations > Ripple Delete Silence (vidcut: the clip's right-click menu or Timeline Actions).
  - settings: threshold (dB), pre-head, post-tail, minimum to trip
  - previews the parts to remove as red stripes; Remove cuts them and closes the gaps, as one undo step
  - details and defaults: D29; the crossfade option is deviation X11

## 10. Quick Export (Ch. 39, pp. 788–790, 4095–4096, 4104–4109)

- **Flow:** File > Quick Export or the toolbar button, then pick a preset from the icon row, then Export, then choose a file name, then a progress dialog.
- **Range:** the timeline In–Out range if one is set, otherwise the whole timeline.
- **Output:** Quick Export always writes one file. Rendering individual clips is a Deliver-page feature in Resolve, which our Export dialog covers.
- **Preset models** (Deliver page):
  - H.264 Master: QuickTime, AAC 320 kb/s
  - H.265 Master: Main10
  - ProRes 422 HQ: LPCM audio
  - YouTube 720p–2160p: MP4, H.264 High
  - Vimeo
  - TikTok: vertical 1080×1920
- **User presets** can be added to the Quick Export row.
- **In vidcut:** the Quick Export row, the Export dialog and jobs are deviation X3. The suggested file name is the project's name. Timeline interchange is deviation X9.

## 11. Keyboard shortcuts

Keys from the Cut chapters, with Edit-page keys filling the gaps (marked †). They are named as on Linux and Windows. On macOS, Ctrl is Cmd and Alt is Option.

Shortcuts do nothing while a text field has focus or a question (a modal dialog) is up, and nothing behind a question sees the keys. Esc dismisses a question without acting: Cancel, or Keep Running on the quit question.

| Key | Action | Page |
|---|---|---|
| Space | Play / stop | 640 |
| J / K / L | Shuttle reverse / stop / forward (repeat = faster) | 696, 871 |
| K+J / K+L, K held + tap J/L | Slow play / single-frame step | 871† |
| ← / → | Step one frame | 870† |
| Shift+← / Shift+→ | Step one second | 870† |
| Up / Down | Previous / next edit | 640 |
| Shift+Up / Shift+Down | Previous / next marker | 689, 718 |
| Home / End | Timeline start / end | 870† |
| I / O | Mark In / Out (source or timeline, by context) | 691 |
| Shift+I / Shift+O | Go to In / Out | 870† |
| Alt+X | Clear In and Out | 700 |
| = / + / − | Go to timecode / relative jump | 688 |
| Q | Viewer: Timeline ↔ last source mode | 696 |
| F9 | Smart Insert | 889† |
| F10 | Overwrite | 701 |
| Shift+F10 | Ripple Overwrite | 890† |
| F12 | Place On Top | 890† |
| Shift+F12 | Append | 890† |
| Ctrl+\ | Split at playhead | 723 |
| Ctrl+B | Split at the playhead; same as the row above, without AltGr | 723 |
| Ctrl+T | Dissolve on the selected edit, or the one nearest the playhead (§8.11) | 758 |
| Delete / Backspace | Delete selection (ripples on Track 1) | 725 |
| Alt+Delete | Lift the selection (leave the gap; D9) | 725 |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / cut (ripple) / paste at playhead | 723 |
| Ctrl+A / Ctrl+Shift+A | Select all / deselect all | 719 |
| Alt+Shift+V | Select clips under the playhead | 719 |
| V / U | Select nearest edit / cycle edit side | 720 |
| , / . (Shift: 1 s) | Trim the selected edit one frame left / right | 870† |
| D | Disable / enable clip video | 724 |
| Ctrl+D | Change clip duration | 693 |
| R | Change clip speed (D48) | † |
| N | Snapping on/off | 716 |
| M / Alt+M | Add-or-edit marker / delete marker at playhead | 718 |
| Double-click an edit | Trim Editor (Esc closes it) | 729 |
| W | Dynamic Trim on/off (then J/K/L trim, Space plays around) | 731 |
| Shift+S | Audio scrubbing on/off | 876 |
| Ctrl+mouse wheel | Zoom the lower timeline (deviation X21) | 853 |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo | 651 |
| Ctrl+I | Import media | 654 |
| Shift+Ctrl+N | New bin | 655 |
| Alt+F / Shift+F / Esc | Enhanced viewer / full screen / exit (later) | 688, 40† |

## 12. Decisions on gaps and contradictions

Each decision applies until the manual or testing shows Resolve behaves otherwise. If testing shows that, update the decision and the code.

- **D1. Fixed playhead.** The lower timeline's playhead is fixed by default, with a Timeline Options toggle. The manual calls it fixed on p. 689 but describes a toggle on pp. 645 and 716.
- **D2. What Split cuts.** If some selected clips are under the playhead, only those are split. Otherwise every clip under the playhead on every unlocked track is split. This is a simplification of the Edit-page Auto Select rule (p. 950). Afterwards, the left halves are selected.
- **D3. Target track.** Smart Insert, Ripple Overwrite and Overwrite use the selected target track (V1 by default; A1 for sound alone, D26). Append always uses Track 1, and Place On Top picks its own track (D24). The manual doesn't say which track Append uses.
- **D4. Trim Start/End to Playhead.** Applies to the selected clips; with no selection, to the Track 1 clip under the playhead.
- **D5. Missing source marks.**
  - No source In means the clip's first frame; no source Out means its last frame.
  - In Source Tape mode with no marks, the edit takes the whole clip under the source-tape playhead.
- **D6. Marks persist after edits.** Source marks are remembered per clip, and timeline marks stay until cleared (Alt+X). This is the least surprising behavior for scripting.
- **D7. Overwrite with both timeline In and Out set.** A three-point edit: the timeline range sets the duration, starting from the source In. If there isn't enough source media, the edit fails with an error.
- **D8. Ripple Overwrite with both timeline In and Out set.** The source duration wins and the program ripples by the difference (p. 698).
- **D9. Lift on Track 1.**
  - With Ripple On off, Delete leaves a gap. Alt+Delete lifts once, consistent with Alt suspending ripple while trimming.
  - The API exposes the same choice as `edit.delete --lift`.
- **D10. Overwrite past the end of Track 1.** With Ripple On, an Overwrite that would start after the end of Track 1 starts at the end instead, so Track 1 stays gapless.
- **D11. Hover time.** 0.8 s of hovering turns a drop or move onto a clip from "replace" into "overwrite". The manual doesn't give a value.
- **D12. Clip gain range.** −60 dB to +24 dB, plus mute. The manual gives no range.
- **D13. Keys.** Shortcuts the Cut chapters don't give come from the Edit page (marked † in §11). Mac → Linux and Windows mapping: Command → Ctrl, Option → Alt. macOS keeps Command and Option.
- **D15. Playhead after an edit function.** After Smart Insert, Append, Overwrite, Ripple Overwrite or Paste, the timeline playhead moves to the first frame after the new clip(s), ready for the next edit. The Viewer stays in its mode, so Q reviews the result (p. 696). The manual doesn't say; this is the Edit page's behavior. A clip dragged onto the timeline leaves the playhead where it was: a drop isn't an edit function, and on the Edit page too only the edit functions move it. Edit functions sent by scripts and agents move it as the buttons do.
- **D16. Mark Out includes the frame under the playhead.** As in Resolve, the Out frame is part of the range. vidcut stores ranges half-open, so pressing O at frame `f` stores `out = f + 1`. The Duration field and `--json` output use the stored values.
- **D17. Source Tape In/Out** (p. 692). I/O in Source Tape mode marks the whole tape: the open bin's clips end to end, in pool order.
  - **Stored:** as tape frames together with that order (`tape.marks.set` takes it), so an edit takes the same span whatever the pool shows later. A span that stays on one clip is also that clip's source marks (D5), which pool thumbnails and Source Clip mode use; a span across clips is only the tape marks. Clearing a one-clip span also clears that clip's marks; clearing a crossing span leaves per-clip marks alone.
  - **Edited:** with `tape: true` instead of `media`, an edit function takes the span: the tail of the first clip, the clips in between, and the head of the last. It places them end to end as one block, by the same rules as one clip: an insert moves what follows by the whole span, Ripple Overwrite replaces the clip (or the marked range) with all of it, Place On Top lines the block up once and puts it on one track, and timeline In and Out (D7) fit the block as a whole. It is one undo step.
  - **Dropped:** removing a clip of that tape, or relinking one to a file of another length, clears the tape marks: their frames would point elsewhere.
- **D18. Drop and move targets.**
  - Within 6 px of an edit point a drop inserts there; the moved clips' own edges don't count.
  - On another clip: replace if released within 0.8 s of reaching it (D11), else overwrite at the block's position. Several clips always overwrite (Replace takes one clip).
  - Past the last clip: the clips move (or the media is appended) to the end. Left of the first clip: inserted at the start.
  - The row under the pointer is the destination track. The strip above the top row means a new video track. The bottom row sits on the timeline's lower edge, and up to 24 px below it means a new audio track, shown there while the drag lasts.
  - Dragged off the tracks (more than 24 px to either side, above the top strip or further below), or Esc during the drag: cancelled, nothing changes.
- **D19. Trimming with the pointer.**
  - Edges follow the pointer's movement from the press (they don't jump to it), snapped to targets (§8.8).
  - A trim, roll, slip or J/L cut stops where the media runs out instead of failing.
  - Safe Edit (p. 732) holds an edge at the neighbor for 0.6 s; pushing on overrides it.
  - Dragging empty space in the upper timeline scrubs; in the lower timeline it draws a marquee.
- **D20. Keyboard trimming.** With an edit selected (V, U, or a click on an edit), `,` and `.` trim it by one frame and Shift+`,`/`.` by one second: the Out, the In, or both (a roll). These are the Edit page's nudge keys (†); the Cut page's own keyboard trimming is Dynamic Trim (D32).
  - **The J/L handle is the audio edit.** A click on it (§8.7) selects that audio edit on the clip's track (D36). The selection's frame is the audio edit, `tail` frames after the picture cut. `,` and `.` move the outgoing clip's `tail` and the incoming clip's `head` together (`edit.audio_roll`). The picture cut stays on its frame. U cycles Out, In and both on a picture edit, and leaves an audio edit selected.
- **D14. J/L cuts belong to an edit point.**
  - A J/L cut (§8.7) is stored as matching audio offsets on the two clips that meet there: the outgoing clip's `tail` equals the incoming clip's `head`.
  - When an edit gives a clip a different neighbor at that point, the J/L cut resets to a straight cut. Examples: a delete, a move, or a split inside the overlap zone, where one half would otherwise be left without audio of its own.
  - A clip can be heard only past its picture: its neighbor's J/L cut covers all of it, and its own sound runs on in its J/L cut at the other end. If that cut resets, the clip would be left with no audio, so the cut at its other end resets too.
  - The manual doesn't cover this. Without the rule, audio could overlap on a track or extend into gaps.
- **D21. Following Track 1** (pp. 649, 727). A clip on another track is attached to the Track 1 picture under its first frame: that clip and source frame. Nothing is stored; each edit works it out from the timeline before and after.
  - When that picture moves (later clips shift, or its clip moves, even to another track), the attached clip moves with it. Split parts count as the same picture.
  - When its clip's tail is trimmed past it, it is deleted (p. 727). When its clip's head is trimmed past it, it moves to the clip's new start.
  - When its clip is replaced (Ripple Overwrite, Replace), it stays where it is.
  - When its clip is deleted, it moves to where Track 1 closed up.
  - Over a gap or past the end of Track 1, it keeps its distance to the next Track 1 clip (or from the end of the one before).
  - Clips on locked tracks don't follow. A clip that would land on another clip moves up to the first free unlocked track of its kind (audio tracks: down), which is created if needed.
- **D22. Rippling other tracks** (p. 721). With Ripple On, deleting or trimming a clip on another track ripples that track only when Track 1 has no clips from that clip's start on. Otherwise it leaves a gap. Deleting a timeline In–Out range takes it out of every unlocked track and closes them together (p. 725).
- **D23. Edit functions on other tracks.** Only Track 1 pushes clips (p. 697). On another track, Smart Insert and insert overwrite, and Ripple Overwrite replaces without moving anything else. Paste puts each clip back on its own track (p. 723), creating tracks that are missing. Split follows D2 on every unlocked track.
- **D24. Where Place On Top goes** (pp. 699–700). Onto the video track above the highest one with clips in the incoming span, created if needed; onto Track 1 only if nothing is there (where D10 applies). Sound alone goes to the first audio track with room, created if needed.
- **D25. Moving clips between tracks.**
  - Moves are given in timeline positions from before the move.
  - A Track 1 clip moved to another track lifts out; with Ripple On, Track 1 closes up and the moved clips land where they were dropped.
  - Clips moved onto Track 1 insert or overwrite there (§8.2).
  - Video can't go onto an audio track. A selection spanning several tracks moves along its tracks only.
- **D26. Sound alone.** An edit without picture (media without video, or the Audio Only toggle, p. 694) goes to A1 unless a target track is given. A clip on an audio track keeps only its audio. In the GUI, sound alone dragged onto a video track lands on the first audio track.
- **D27. Track numbers are positions.** Video tracks are V1, V2, … from the bottom; audio tracks A1, A2, … from the top. Removing a track renumbers the ones after it. Edits and moves may name the next track (one past the last) to create it. Track 1 can't be removed, and removing a track with clips must be confirmed (`--force`). Track volume runs from −60 dB to +24 dB, like clip gain (D12).
- **D28. Meters.** Each meter is one bar per channel of the project's channel count (`settings.channels`; stereo is the usual case), as on p. 649. Peaks are linear and drawn from −60 dBFS to 0. Each bar holds its peak for 1.5 s.
  - **Levels:** while the viewer plays, the bars show the player's sound as it is heard: the mix, and each track's lane with its fader, peaks over the last 50 ms. So a fader being dragged, another speed and backward play show as they sound. Paused or scrubbing, the bars read the media waveforms at the playhead, one peak per channel per 10 ms, with clip gain and the track fader, so they move then too. A muted clip is silent. Mute and solo decide which tracks are heard in the mix and which strips show silence, the same way they apply to the export (p. 650). The mix sums the audible tracks per channel, as the preview mixes them. The viewer's monitoring mute (`player.mute`) silences the viewer meter only.
  - **Channels:** each waveform's channels are the project's, mixed the way the export mixes them, so a 5.1 source's centre and surrounds feed a stereo meter's left and right, and a mono source lights every bar. The mix can pass full scale when tracks sum.
- **D29. Ripple Delete Silence** (p. 773). The manual gives no defaults or measuring rule.
  - **Silence** is where every channel's peak, after the clip's gain, stays below the threshold, measured in 10 ms steps. Parts of the clip past the end of its audio count as silent. So does a click: a sound shorter than 0.05 s with at least 0.1 s of silence on both sides.
  - **Defaults:** threshold −40 dBFS, pre-head and post-tail 0.15 s, minimum to trip 0.5 s. They are seconds rather than frames, so a project's frame rate doesn't change what they mean.
  - **What goes:** each silence at least the minimum long, less the post-tail after the sound before it and the pre-head before the sound after it. Silence at a clip's start or end goes entirely. Only whole frames are cut.
  - **One edit:** it ripples whatever Ripple On says, as its name promises. Track 1 closes up, and clips on other tracks follow (D21). It is one undo step, and a dry run reports the parts without cutting (the red stripes).
- **D30. Finding shots and black.** Detect Scene Cuts (p. 690) gives no algorithm.
  - **Shots:** each frame gets FFmpeg scdet's scene score on its luma: the mean difference from the previous frame (0–100), or the change in that difference if smaller, so steady motion doesn't read as a cut. A frame scoring at least 10 starts a new shot. The cut lands on the first timeline frame that shows it.
  - **Black:** a frame is black when 98% of its pixels are within 10% of black. Black stretches of 0.5 s or more are reported (`analyze.black`).
  - **Measurement:** pictures are measured at 96×54 and sounds in 10 ms peaks. Measurements are cached per file, so trying other thresholds is instant.
- **D31. Boring Detector defaults** (p. 690). The manual gives none. Clips over 30 s are boring and clips under 10 frames are jump cuts (2 frames finds flash frames, p. 690). The detector looks at clips on video tracks, and its settings are view options kept between runs, not part of the project.
- **D32. Dynamic Trim** (pp. 730–732).
  - **Running:** L or J runs the trim at the shuttle speed (repeat for 2×, 4×, 8×), as playback would move. The run stops where the media or a neighbor runs out, and shows it: the selected cut turns red and the viewer says why.
  - **Seeing it:** the viewer shows the two-up: the frames on both sides of the edit, or of the slipped or sliding clip, as the trim leaves them (exact frames, as the Trim Editor shows). The timeline draws the clips as the trim leaves them, so the cut moves while the run goes on.
  - **The selection:** the selected edit follows the trim. An undo, or someone else's edit, that takes the selected cut away clears the selection.
  - **Applying it:** K stops the run and applies it as one request (`edit.trim`, `edit.roll`, `edit.slip`, `edit.slide`, or `edit.audio_roll` when the selected edit is the J/L handle), also a run that stopped by itself. K held with a tap of J or L applies one frame. W, Space and Up/Down also apply what is pending.
  - **Play around:** Space plays from 2 s before the selected edit, or the first selected clip's start, to 2 s after it.
  - **From scripts:** `gui.dynamic_trim` turns it on and off, picks the side or Slip and Slide, runs it and steps it. `player.state`'s `two_up` says what the two-up shows, and how many of the frames asked for were passed over.
- **D33. Rotation metadata.** The manual doesn't say. Media show as players show them: the rotation in a file's display matrix is applied in the preview, export, frames, thumbnails and storyboards. Phones store portrait video sideways with one. A portrait clip that sets the project format makes a portrait project, and `media.get` reports the size as shown, with the rotation. Mirrored display matrices are taken as rotations only.
- **D34. Interlaced sources** (Deinterlace Quality, p. 741).
  - **Detection:** a stream is interlaced if its field order says so, or if its first frame is flagged interlaced. `media.get` reports the field shown first as `fields`.
  - **Deinterlacing:** frames flagged interlaced are deinterlaced everywhere (export, frames, storyboards, the preview), one progressive frame per frame. The first field (in time) is kept and the second rebuilt from it and the neighboring frames (`bwdif`). Like the Cut page's playback, which always uses Normal quality, there is one method (deviation X13).
  - **Rates:** output is never interlaced, and an interlaced clip in a project at twice its frame rate shows each frame twice (no field-rate output).
- **D35. HDR and color** (Resolve's color management, pp. 230–239).
  - **Detection:** a clip is HDR when its transfer is PQ (SMPTE ST 2084) or HLG; `media.get` reports it as `hdr`.
  - **SDR output:** HDR clips are tone-mapped to BT.709. Up to 80% of the 203-nit reference white stays as it was, and brighter highlights roll off, up to 1000 nits. Which output an export gets is D44.
  - **HDR output:** PQ or HLG (`video.hdr`, chosen by D44 or set) writes BT.2100 in 10 bits. SDR clips are placed with their white at 203 nits (BT.2408), as Resolve's "203 nits" option does (p. 238).
  - **Tags and untagged media:** outputs are tagged with their colors. Untagged media are BT.709 when 1280 wide or over 576 lines, and BT.601 otherwise, as players commonly guess; the viewer reads them so too (a 1280×534 widescreen film is HD).
  - **Preview:** the viewer's shader converts and tone-maps with the export's math, so HDR clips look as in an SDR export (deviation X14).
- **D36. Which track an edit point is on** (pp. 696, 720, 729–732). The manual pictures one main story.
  - **Track 1's edits:** the Smart Indicator, V (select the nearest edit), Smart Insert, Place On Top's alignment and Up/Down navigation use Track 1, the main story.
  - **A clicked edit keeps its track:** an edit picked by clicking a clip's edge, a cut, or the J/L handle on that cut stays on that track. The handle selects the audio edit (D20); an edge or a cut selects the picture. `,`/`.`, U, the Trim Editor (its labels and frames are that track's clips, whatever covers them) and Dynamic Trim work there. Dynamic Trim's Up/Down move along that track's edits, and an audio selection stays on the audio edit of the cut they land on.
  - **During Dynamic Trim:** `,` and `.` do nothing while it runs. Its pending trim is the one request K sends (D32).
- **D37. The format: chosen at the start, or set by the first video without asking** (§3; Resolve offers it, p. 654).
  - **By the first video:** in a project created without a format, the first media file with video sets the frame rate and the picture size as shown (D33), with no prompt: a dialog would stop scripts and agents.
  - **Chosen:** New Project (and `project.create` or `project.open` with `create`) takes a picture size and a frame rate. The size is any even width and height from 16 px; the rate is any positive rate, `num/den` or a decimal (12.5, 29.97, 30000/1001), not only the common ones the list offers (Resolve allows a fixed set).
  - **Changed later:** File > Project Settings (`project.set`) changes the size at any time, since every clip is fitted to the frame when it is drawn. The frame rate changes only while the timeline is empty: timeline positions are whole frames at that rate.
- **D38. Stills** (§3; pp. 114, 1194). The manual gives two lengths for a still: the standard still duration, 5 s by default (p. 114), and "10 seconds" for imported stills (p. 1194). vidcut follows the preference's 5 s.
  - A still edited without a range or marks lasts the project's still duration (`project.set still_duration`, deviation X16). The Viewer shows it as a clip that long.
  - Its clips can be made longer by a range or a trim, up to 17 h 40 min (p. 1194). Every frame shows the one picture, so a still has no frame rate, and the first video still sets the project format (D37).
  - Media that are images: what FFmpeg reads as one (its `image2` and `*_pipe` demuxers).
- **D39. Image sequences** (§3; p. 366). Numbered files (`frame_000001.png`, `frame_000002.png`, …) come in as one clip with `media.add sequence`: the files numbered in a row around each file given, with no number missing. Its frame rate is the project's unless `fps` says otherwise (Resolve takes it from Clip Attributes).
  - Numbers zero-padded to a width (`%06d`) or not (`%d`) both work; the width shows in the file names.
  - The pool calls it as Resolve does: `frame_[000001-000120].png`.
  - Without `sequence`, a numbered image is a still (deviation X17).
- **D40. Timeline interchange with Resolve** (deviation X9; pp. 527–530, 4160). The manual describes Resolve's OTIO import and export, not the files. Decided from files Resolve exports and from what its import was seen to need:
  - **Source positions are timecode.** A source range counts from the media file's start timecode (a `tmcd` track, MXF's, a broadcast WAV's time reference), as Resolve writes and reads it; the available range starts there too. Media without one start at 0. Drop-frame timecode counts its skipped numbers out.
  - **Paths are plain.** Resolve reads `target_url` as a path: `file://` URLs and `%20` escapes don't find the file. vidcut writes plain absolute paths, and reads URLs, escaped or not, too. In bundles, paths are relative.
  - **Audio stays with its video** through Resolve's link group number, written on both and read when there.
  - **Marker notes** are written in `Resolve_OTIO.Note` as well as `comment`. A marker keeps its length, one frame unless given one (`markers.add` and `markers.set` take `duration`; OTIO's marked range carries it), though the timeline draws it as a point. Track names are deviation X9 (a set name is written and read back; the place defaults `Video 1` and `Audio 2` are not a name someone gave). A timeline name has no field the import was seen to keep, and Resolve's export writes none.
  - **Bundles:** `.otioz` as OpenTimelineIO's adapter writes it (what Resolve imports); `.otiod` the same as a folder. Unpacked into a folder named after the bundle, as Resolve does (deviation X9).
- **D41. Proxies** (pp. 207–218 are outside the Cut page; deviation X18). The manual's proxy chapters are not what the Cut page specifies, so the playback rules are fixed here.
  - **What gets one:** video and image sequences, generated on demand (`proxy.generate`). Stills and audio-only media do not. Naming a still or an audio-only file is an error; generating with no ids skips them.
  - **Codec:** the preferences' `proxies.codec`.
    - `dnxhr` makes DNxHR LB, which is at least 256×120 (a smaller proxy is an error).
    - `prores` makes ProRes 422 Proxy: on Apple's ProRes hardware (VideoToolbox) when there is one, else on the GPU in use with FFmpeg's Vulkan encoder (FFmpeg 8.1 and later), else with FFmpeg's CPU encoder.
    - `av1` makes AV1 on a hardware encoder (NVENC, then VAAPI), else with SVT-AV1.
    - `hevc` makes HEVC on a hardware encoder (NVENC, AMF, VAAPI, QSV, VideoToolbox), else with x265.
    - `h264` makes H.264 on a hardware encoder (NVENC, AMF, VAAPI, QSV, VideoToolbox), else with x264.
    - Where FFmpeg lacks the codec's software encoder (SVT-AV1, x265, x264): H.264 with x264, else OpenH264, else ProRes Proxy, which every FFmpeg has.
    - A source of more than 8 bits keeps 10 where the encoder takes them: AV1 and HEVC (SVT-AV1, x265 where its FFmpeg's takes 10 bits, and hardware that takes P010), and ProRes, which is always 10-bit. H.264 and DNxHR LB are 8-bit.
    - A source with 4:2:2 color or more keeps it in ProRes and DNxHR LB proxies. Apple's ProRes hardware takes 4:2:2 pictures where the encoder check finds it working with them. Where it doesn't, it gets 4:2:0 pictures and is named `ProRes Proxy from 4:2:0`. AV1, HEVC and H.264 proxies are 4:2:0.
    - `auto` (the default) goes through DNxHR LB, ProRes Proxy, AV1, HEVC and H.264 in that order, four times. First for one the GPU makes and plays (the player decodes it there). Then for one the GPU plays, then for one the GPU makes (through Vulkan too), then for one made in software. ProRes Proxy comes before AV1 and HEVC: every frame is a keyframe, so a jump decodes one frame, and a Mac's GPU and NVIDIA's show its frames from GPU memory. On NVIDIA that is ProRes Proxy through Vulkan, which takes the decoder's frames on the GPU and counts as made there where the GPU plays ProRes on Vulkan. On a Mac it is ProRes Proxy on VideoToolbox. On AMD's and Intel's GPUs it is HEVC, or AV1 where VAAPI makes it. Without a GPU it is DNxHR LB. Where another decoder plays ProRes, ProRes made through Vulkan counts as played only. Every proxy has a closed GOP of 60 frames and codes each frame in the order it shows. ProRes and DNxHR make every frame a keyframe, so a jump decodes one frame, in files many times larger (preview.md §6). The codec is part of the file name.
  - **Color:** an 8-bit proxy is SDR BT.709, HDR tone-mapped as an export tone-maps it. A 10-bit proxy keeps an HDR source's PQ or HLG in BT.2020, and a PQ source's static HDR metadata (mastering display, light levels), written as an export writes them. The viewer tone-maps it as it does the original. HDR10+ and Dolby Vision don't go into proxies: a proxy is for playback, and exports read the original.
  - **Size:** the longest side is the preferences' proxy size (`proxies.size`, 960 pixels unless changed), even, from 16 to 7680; `width` overrides it. Aspect is kept. A picture already smaller stays its size.
  - **Automatic:** with `proxies.auto` (on by default), while `use_proxies` is on, the running GUI makes proxies by itself for the media the rules pick: image sequences (`proxies.image_sequences`), videos with more pixels than `proxies.larger_than` (3840×2160: a GPU plays 4K), and video files averaging more than `proxies.above_mbps` (150 Mb/s: camera and editing formats a GPU can't decode). It checks the media an import adds, and the whole pool when `use_proxies` is turned on, never otherwise: a proxy someone removed stays removed until then. The request is the session's own `proxy.generate` with `auto: true` and the media named (origin `session` in the log), a job like any proxy encode; media already being encoded are left out. Headless runs never start encodes by themselves: scripts and agents don't need proxies. `auto: true` on any `proxy.generate` (`vidcut proxy generate --auto`) takes only what the rules pick and has no fresh proxy; finding nothing is fine.
  - **Encoding** runs beside the session, like an export: progress is shown, cancelling (Ctrl-C, or a client that leaves) stops it and leaves no file, and the proxies are recorded on the project once their files are written. The GUI keeps working meanwhile, and a headless run takes the project's lock only to record them. In a running GUI the encode is a job (gui.md §9): the Jobs window, which Generate Proxy opens, shows its progress and cancels it, and so does `jobs.cancel`.
  - **Switch:** `use_proxies` (saved on the project) makes the preview play a proxy. Export, frame render, analysis and OTIO always use the original paths.
  - **Fresh:** a proxy is used only when its stored source size and modification time match the pool item and the file is present. Otherwise the original plays. Relink drops the proxy. An image sequence's size and time are the whole run's, read once when it is added or relinked: the total of its pictures' sizes and the newest of their times, so a render that rewrites later pictures counts as a changed file. Nothing is read from disk when playback starts.
  - **Frame exact:** proxy frames carry the source's presentation timestamps. The preview keeps `origin` and `source_frame` and only replaces the video path, so a sequence plays as that video and the export's sampling rule picks the same frames. Preview audio stays on the original.
- **D42. Audio scrubbing** (§4.5; p. 876). The manual says only that dragging the playhead plays the sound when scrubbing is on, as it is by default.
  - **What plays:** a frame step (←/→, K held with J/L tapped) and every move of a paused playhead (a drag is many) play the sound of the frame arrived at: its samples of the mix, every audible track with its gains and fader, faded in and out over 3 ms so it doesn't click. The monitoring mute silences it. Playing, nothing extra plays.
  - **Forward, either way:** a step back plays its frame forward too. A frame is 33–42 ms: reversed, it isn't heard as backward, and forward the start of a word stays recognizable.
  - **One at a time:** a frame's sound plays whole. A newer one waits for it and replaces any still waiting, so a held key plays frame after frame and nothing piles up behind it.
  - **Playing backward** plays the sound reversed (J), at other speeds keeping its pitch as forward play does.
- **D43. Enlarging a track** (pp. 647, 678). The manual says a track's sizing button enlarges it, showing its pictures and sound full size, one track at a time.
  - **Size:** the enlarged track is three times its usual height, and at least 180 px. In a timeline too short for that, the other tracks shrink first, down to one line each, and the enlarged track takes what they leave.
  - **View, not edit:** it is how the lower timeline shows the tracks, like its zoom: no request, nothing in the project or the undo history, and it ends with the window. The upper timeline doesn't change.
  - **Buttons:** the header's buttons are icons that show the state: a lock closed or open, a speaker and an eye crossed out when off. A button is lit when its track is out of the ordinary: locked, muted, video off, enlarged.
- **D44. An export's dynamic range and bit depth follow its footage** (§10; deviation X14). The manual's exports deliver what the project's color management is set to; vidcut has none, so each export decides from the pictures it shows, unless told.
  - **What counts:** the frames the export shows from sources: the visible top clip at each frame, in the range, by duration. Black (gaps) doesn't count. A per-clip export decides for each file from its own clip.
  - **Dynamic range (`video.hdr`: `auto`, the default):** HDR when more than half of that picture is HDR and the codec carries HDR (HEVC, AV1, VP9) with an encoder here that writes 10 bits; HLG when every HDR frame is HLG, otherwise PQ. Anything else is SDR, the HDR frames tone-mapped (D35). `sdr`, `pq` and `hlg` set it.
  - **Bit depth (`video.bit_depth` unset):** 10 for HDR. For SDR, 10 when more than half of the picture has 10 bits or more and the codec's 10 bits play widely: HEVC Main10, AV1, VP9 profile 2. H.264 stays 8: High 10 plays in few places. ProRes is 10-bit, DNxHR as its profile is, and DNxHR HQ becomes HQX, its 10-bit form at the same data rate. SDR takes 10 bits only from the encoder an 8-bit export starts with (a GPU's, for `auto`): if it can't write them, 8, rather than a slower encoder. A set depth or a profile that fixes one wins.
  - **Presets:** the masters (H.265, AV1, VP9) and the H.264 ones are `auto`, so H.264 is SDR, with the note below; TikTok asks for `sdr`, shown on phones.
  - **Said, quietly:** the plan and the result report each file's choice, what it came from (frames of PQ, HLG and 10-bit picture) and why. When HDR frames will be tone-mapped and SDR wasn't asked for, the plan carries a note with the overrides that keep them HDR (HEVC when the codec can't). The Export dialog shows it as one dim line under the plan with an Export HDR button that applies them; never a question, and Quick Export and the export go on as they are. With `sdr` set, nothing is said.
- **D45. Passes** (§10; Resolve's multi-pass option is on the Deliver page, outside the Cut page). Two passes bring an export closer to a target bitrate and spread the bits better; at constant quality they change nothing worth the time.
  - **`video.passes`:** 1 or 2. Unset, 2 with a target bitrate and 1 with constant quality, where the encoder makes two. Two need a bitrate.
  - **Who makes two:** x264, x265 and VP9 encode the range twice, the first time only to measure; NVENC looks at each frame twice within one encode. VAAPI, AMF, QSV, ProRes, DNxHR and (in FFmpeg 8.0) SVT-AV1 make one; two asked of them is an error, and `auto` moves on to an encoder that makes them.
  - **Settings, with their defaults:** `first_pass` `fast` (quicker settings for the measuring run; `full`), `multipass` `quarter` (NVENC's first look at a quarter of the resolution; `full`), and `lookahead` 32 frames on NVENC whatever the rate control (0 to 32; 0 for none).
  - **While it runs:** progress says which run it is; cancelling either leaves nothing; the statistics, in a hidden folder beside the output, are removed however it ends. A per-clip export makes both for each file. The Export dialog shows the passes at a bitrate, with the first-pass speed or NVENC's settings for the encoder the plan names.
- **D46. HDR metadata follows the footage** (§10; deviation X19). The manual's HDR exports carry what the grading setup says (the Mastering Display, Export/Embed HDR10 Metadata, p. 4117); vidcut doesn't grade, so it carries what the sources say.
  - **Read** when media are added: the mastering display (SMPTE ST 2086), the content light levels (MaxCLL, MaxFALL), whether frames have HDR10+ (ST 2094-40), and the stream's Dolby Vision profile. HDR media whose entry has none of these are probed again at export.
  - **Static values (`video.hdr_metadata`: `auto`, the default), PQ output only:** the widest color volume of the pictures shown (primaries that hold all of theirs, the highest peak, the lowest black) and their highest MaxCLL and MaxFALL. SDR pictures count as BT.709 at 203 cd/m², with light levels of 203; PQ ones without a mastering display as BT.2020 at 1000 cd/m², the peak vidcut's tone mapping assumes; HLG ones as BT.2020 at 1000 cd/m², HLG's reference display. `video.mastering_display` and `video.max_cll` (x265's forms) set them. HLG and SDR output carry none.
  - **Measured light levels:** a picture without its own (a PQ file without them, HLG converted into PQ) is measured in the frames the export shows, as the export converts them: MaxCLL is the brightest channel of the brightest pixel, MaxFALL the highest frame average of each pixel's brightest channel. The averages, and the percentiles HDR10+ is made with, are of at most half a million pixels of a frame, which estimates them closely. `video.max_cll=measure` measures every picture instead of reading the files. Measuring happens when the export starts, before encoding, and progress says so; a plan says the levels will be measured then. Only what the export shows of a file is measured, and the result is kept, so the next plan or export of the same footage has it at once.
  - **Dynamic metadata:** HDR10+ and Dolby Vision describe each frame's own picture, so each output frame keeps those of the frame it shows, where its transfer stays the same. HDR10+, PQ output only, is written when any picture shown has it: frames without it get HDR10+ made from their measured light (each channel's peak, the average and percentiles, no tone mapping curve), black frames that of no light. Dolby Vision is written when every picture shown has it, one profile: 8.1 into PQ, 8.4 into HLG (10 for AV1). An encoder writing it needs it on every frame, so black frames take the Dolby Vision of the frame before them, and black before the first picture leaves it out. libx265 and libsvtav1 write it, so `auto` then encodes on the CPU. `static`, `hdr10plus`, `dolby_vision`, `all` and `none` set what is written; `hdr10plus` makes HDR10+ for every frame when no picture has its own.
  - **Where:** in the bitstream (HEVC SEI at every keyframe, AV1 metadata OBUs; HDR10+ on every frame) and in the container (MP4/MOV `mdcv`/`clli`, Matroska/WebM's colour metadata, Dolby Vision's configuration; VP9's HDR10+ as WebM BlockAdditions).
  - **Said:** the plan and the result report each file's metadata and why (what was left out, and for what); the Export dialog shows it in one dim line, and offers the setting next to the dynamic range.
- **D47. Drop-frame timecode** (§4.3; Resolve's project settings offer it at these rates). At 29.97 and 59.94 fps a project can count record timecode drop-frame: frame numbers 00 and 01 (00–03 at 59.94) are skipped at the start of every minute but every tenth, so `01:00:00;00` is an hour of the clock, frame 107892, where non-drop `01:00:00:00` is 108000.
  - **Setting:** `project.set drop_frame` (`--drop-frame true`), and `project.create` and `project.open`'s create; New Project and File > Project Settings show it at those rates. Refused at other rates; a change to a rate without it turns it off.
  - **Shown** with `;` before the frames wherever timecode is: the viewer, the ruler, storyboards, the `*_tc` of query results, lengths and source positions too. A media file's own timecode shows as it counts.
  - **Typed:** `;` says drop-frame anywhere (`01:00:00;00`, `01;00;00;00`); in a drop-frame project `:` counts so too. A label drop-frame skips (`00:01:00;00`) is refused. Digits typed into a timecode field that fill such a label (`10000`, a minute) land on the minute's first frame (`00:01:00;02`).
  - **Set by itself:** the first video of a project whose format it sets (D37), and an OTIO import into an empty timeline, turn it on when the media's own timecode counts drop-frame (written with `;`). OTIO doesn't say whether a timeline counts so: only frame numbers travel.
- **D48. Clips at another speed** (§1, §8.6; deviation X9). A clip's constant speed, set with Change Clip Speed or from an OTIO file, plays wherever its pictures and sound go (the viewer, exports, frame renders, storyboards and analyses), and edits go through it.
  - **Setting** (`edit.change_speed`; R, Edit Actions and the clip's menu), as Resolve 21.1.1's Change Clip Speed does: without ripple the clip keeps its length, forward from where it started or, turned around, from the frame it ended on; with ripple it keeps its source, its length the source over the speed (99 frames at 50% become 198), later clips following. A freeze holds the frame under the playhead. Either way it ends where its media does. Speeds from 1% to 10000% either way, or 0.
  - **Pictures:** each frame of the clip shows the source frame its stretch of the source starts in, by the sampling rule (export.md §2): every other frame at 200%, each twice at 50%, its first frame held at 0 (a freeze). Backward, a stretch starts at its bottom, as Resolve shows it: at −200% from a top at 150, frames 148, 146, …. No frame blending or optical flow.
  - **Sound:** the sound of the frames shown, stretched keeping its pitch; backward reversed, from the top of the first frame's stretch. A freeze is silent.
  - **Analyses:** Ripple Delete Silence, Detect Scene Cuts and the analysis queries find a clip's silences, shots and black through its speed (`analysis::Span`); a freeze has none.
  - **Editing:** trims, rolls, slides, splits and J/L rolls count timeline frames through the speed, and so do media limits. A clip keeps where in its pace it starts, so an edit leaves every frame showing what it showed: a 50% clip split at its 25th frame starts its second part half a source frame in. A slip moves the content by frames of the clip (two source frames at 200%).
  - **Interchange:** a reversed clip's OTIO source range starts at the bottom of what it shows, with Resolve's `Media In`, the room above its top in seconds of timeline, which Resolve places it by (deviation X9). Read from Resolve, a reversed clip at a fraction of normal speed pairs its frames as Resolve shows them; vidcut's own file keeps each clip's pace (`metadata.vidcut`). A speed set in vidcut is written as OTIO's `LinearTimeWarp`, a freeze as `FreezeFrame`.
  - **Compound clips:** the clips one holds play at their own speeds. A compound clip at a speed plays each where it shows it, through its pace, at both speeds together, backward in reverse order; Decompose in Place leaves them so.
- **D49. What Join Match Edit Clips joins** (pp. 646, 950). The manual says only that it rejoins the parts of a split clip.
  - Two clips join when the second plays on from where the first stops, so joining changes nothing you see or hear. They sit side by side on one track, with the same media, speed and settings and no frames missing between them. A fade or a carried transition where they meet keeps them apart.
  - With two or more clips selected, the ones that meet join. Otherwise the clips that meet at the playhead join: the selected ones there, or else those on every unlocked track, as with Split (D2).
  - A run of parts becomes its first part, which keeps its id.
- **D50. Dissolves** (§8.11; deviations X9, X24). The manual says only that the outgoing picture fades as the incoming one comes in. How each frame looks and sounds was measured on Resolve Studio 21.1.1's renders.
  - **Levels:** frame i of an n-frame dissolve shows the incoming picture at (i + ½)/n of its level and the outgoing one at the rest (0.05, 0.15 … 0.95 for 10 frames). They are mixed in the output's code values (YUV, or RGB for a picture render), not in linear light, as Resolve's measure.
  - **Gaps:** a dissolve into or out of a gap mixes the clip with what is under the track there: black on V1, a lower track's picture elsewhere.
  - **What plays:** OTIO's `SMPTE_Dissolve`, unless the editor metadata names another kind of dissolve, and a sound's Cross Fade, by its effect type (41 +3 dB, 40 0 dB, 26 −3 dB, which Resolve's import goes by) or name. Another editor's `SMPTE_Dissolve` on sound plays at 0 dB. A transition plays only where its frames hold nothing of the track but its two clips, and not over another's frames. Any other transition plays as a cut (deviation X9).
  - **Cross fade curves:** at x of the way, the incoming sound's gain is √x (+3 dB), x (0 dB) or x^1.5 (−3 dB), and the outgoing one's the same of 1 − x. It starts from silence at the fade's first sample and covers the transition's frames exactly. Each sample follows the curve (deviation X24).
  - **De-click:** a cross fade's own edges get none, as in Resolve.
  - **Program length:** a fade to black past the last clip lengthens the program to its end, as Resolve's timeline is.
  - **Made here:** a second, centered with an odd length's middle frame before the edit, as Resolve centers one. Each side is shortened to the frames its clip's media and the track leave, and with none on either side it is an error. A new one replaces the transition on its edit. The sound gets a Cross Fade +3 dB where the two clips' sounds meet at one frame (no J/L offset between them), or a fade at a gap. A J/L cut's sound gets its own with Ctrl+T on its audio edit. They are written as Resolve writes its own, and Studio 21.1.1 rebuilds them: the same types, lengths and places, and its render of them within 10-bit rounding of vidcut's.
  - **Viewer:** each picture is drawn over the ones before at its level, after its conversion to the viewer's RGB: within a level of the export's mix in SDR. An HDR export mixes in its PQ or HLG code values, and the viewer shows that picture tone-mapped, so mid-dissolve it is not the export's mix tone-mapped ([todo](todo/hdr-dissolve-preview.md)).
  - **On the GPU:** NVIDIA's GPU mixes NVENC's pictures with the CPU's sums. Where its driver won't take the kernel, the CPU mixes them and the export's report says so. Frames for Direct3D 11's, VA-API's, VideoToolbox's and Vulkan's encoders come down to be mixed on the CPU ([todo](todo/gpu-dissolves.md)).
  - **The two-up** shows the clips without their dissolves.
