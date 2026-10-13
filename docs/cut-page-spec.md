# Cut page behavioral spec

vidcut copies the behavior of DaVinci Resolve's **Cut page** by default. This document describes that behavior in our own words, with page references to the *DaVinci Resolve 21.1 Reference Manual* (the Cut page is Chapters 33–39, pp. 635–790). Printed page numbers equal PDF page numbers. Where vidcut differs: [deviations.md](deviations.md). §12 settles what the manual leaves open.

**Reading the manual:** the PDF is 183 MB; extract text with `pdftotext -f 635 -l 790 <manual>.pdf -`. The developer's copy is in `$VIDCUT_TEST_DATA` (default `~/Downloads/vidcut-tmp`). Don't copy manual text into the repo: describe behavior in your own words, with page numbers.

**Keys:** the manual uses Mac keys. On Linux and Windows, Command → Ctrl and Option → Alt.

**Markers:** **(later)** a Cut-page feature not built yet; **(out)** out of scope (§1).

## 1. Scope

**In scope:**
- media pool, viewer, dual timeline
- editing and trimming
- clip/track audio levels and waveforms
- Quick Export, plus our Export dialog

**Out of scope (out):**
- effects, transitions, titles, generators, keyframes
- color, Fusion, Smart Reframe, Close Up
- speed changes, Sync Bin, Multi Source, Source Overwrite, Live Overwrite
- Replay, voiceover, subtitles
- AI tools: scene detection, transcription, voice/dialogue tools
- Mixer FX, Paste Attributes, proxies, guides
- interchange formats: DRT, AAF, XML

## 2. Layout (pp. 636–637)

- **Main areas:** Media Pool on the left, a single Viewer on the right, and the timeline area across the bottom. The pool/viewer seam and the timeline height can be resized.
- **Top bar:** Media Pool tab, Mixer tab, Quick Export, Full Screen viewer. (Sync Bin, Transitions, Titles and Effects tabs and the Inspector: out.)
- **Below the Media Pool:** the edit-function buttons (§7).
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
  - **Upper timeline:** track-number buttons to its left select the target track; the Audio Only and Video Only toggles sit there too (pp. 643, 694).

## 3. Media Pool (Ch. 34, pp. 653–675)

- **Import** (p. 654):
  - Ctrl+I, the Import Media button, or right-click > Import Media opens a file dialog with multi-select.
  - Import Media Folder imports a whole folder.
  - The first import into an empty project offers to change the project frame rate to match the media (vidcut: D37).
  - File dialogs start where they were last used, remembered across runs: Import where media last came from (else the project's folder), Open Project and New Project where a project was last opened or created, exports where the last export went.
- **Stills and image sequences** (pp. 114, 366, 1194):
  - An image file (PNG, JPEG, TIFF, EXR, DPX, …) comes in as a still. Edited without a range or marks, it lasts the standard still duration. Its clips can be up to 17 h 40 min long (D38).
  - Numbered image files (renders, scans) can come in as one clip, an image sequence, at a frame rate (D39).
- **Remove** (p. 654): Delete or Backspace, after a confirmation. Only the pool entry goes; files on disk stay.
- **Views:**
  - **Thumbnail** (p. 664): hovering scrubs the clip; I/O while hovering marks source In/Out; clicking the thumbnail's lower-right corner shows its metadata.
  - **List** (p. 665): sortable columns.
  - **Filmstrip** and **Metadata** cards (later).
- **Sort and search** (pp. 666–667):
  - Resolve's Sort Media By offers Clip Name, Date Imported, Date Modified and Timecode, ascending or descending. vidcut sorts by date added (the default), name, duration (not in Resolve's list) or file date, ascending or descending.
  - The sort order is also the Source Tape order.
  - The search field filters as you type. It searches the open bin and the bins inside it.
- **Bins** (pp. 655–656, 377):
  - The Master bin is the top level. The pool shows the open bin's own clips, and the bins inside it as folders. A path at the top (Master › Interviews › Day 1) opens any level; the Bins menu lists every bin as a tree.
  - Double-click a bin to open it. New Bin is on the File menu (Shift+Ctrl+N), the Bins menu and the pool's right-click menu. Right-clicking a bin offers Rename Bin and Remove Bin.
  - Drag clips onto a bin, or onto a level of the path, to move them into it. A clip's menu offers Move to Bin and Create Bin With Selected Clips ("Bin N", p. 656).
  - Drag a bin onto another bin (a folder or a level of the path) to move it, with everything inside, keeping its name (`bins.rename`). Dropping it on itself, on a bin inside it, or on its current parent does nothing.
  - Removing a bin keeps its clips (deviation X6).
  - Imports go into the open bin (p. 377).
  - Source Tape strings together the clips of the open bin and of the bins inside it, in sort order (pp. 680, 696).
- **Clip color** (later) (pp. 667–668).
- **Relinking media** (pp. 668–669):
  - A pool item whose file is missing is marked MISSING (red in the list). Its timeline clips are red (pp. 509, 530), and the viewer shows Media Offline with the file's name where it would show them.
  - The Relink Media button, shown while files are missing, finds them in a chosen folder and its subfolders. A clip's menu offers Relink… for one file. vidcut checks each file it finds (deviation X1).
- **Interaction:**
  - **Click:** selects. In Source Clip mode the Viewer shows the clip; in Source Tape mode the playhead jumps to the clip's first frame (pp. 680, 704).
  - **Double-click:** opens the clip in the Viewer as Source Clip (p. 680).
  - **Drag onto a timeline:** a drag-and-drop edit (§7.3).
- **Selection** (p. 719): click, Shift-click range, Ctrl-click toggle, marquee, Ctrl+A.

## 4. Viewer (pp. 639–640, 679–693, 713)

### 4.1 Modes
- **Source Clip** (p. 680):
  - shows the selected pool clip
  - a scroll area under the picture stands for the whole clip, with the playhead, a zoomed-in waveform and draggable In/Out handles
  - Jog In and Jog Out controls at its ends nudge the marks by single frames
- **Source Tape** (pp. 680–681, 704–706):
  - all pool clips play end to end as one continuous tape, in pool sort order
  - selection is synced both ways: the clip under the playhead becomes selected in the pool, and clicking a pool clip moves the playhead to it
- **Timeline** (p. 681):
  - shows the timeline at the timeline playhead; there is no scroll area
  - icons mark a clip's first and last frame
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
  - Home/End: start/end. Shift+I / Shift+O: go to In / Out (p. 870).
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
  - drop-frame at 29.97 and 59.94, in a drop-frame project (D47)
  - Timer 1/2 and KeyKode (later)

### 4.4 In/Out (pp. 691–693, 700)
- **Source In/Out:** I/O in a source mode or while hovering a pool thumbnail, or drag the scroll area's handles.
- **Timeline In/Out:** I/O in Timeline mode, or while dragging either timeline's ruler. The range is drawn on both rulers and can be dragged there.
- **Clearing:** Alt+X clears both marks.
- **Duration field (top right):** shows the marked source duration. Editing it moves the Out mark; it takes frames, ±frames or a timecode (p. 693).

### 4.5 Other viewer features
- Full Screen (Shift+F) and Enhanced viewer (Alt+F) (later).
- Safe areas (later); Fast Review (later).
- Skimming with C held (later).
- **Audio scrubbing** (p. 876; on by default): dragging the playhead, and frame steps, play the sound under it (D42; the player engine's `audio_scrubbing`). Shift+S, Timeline > Audio Scrubbing and `player.audio_scrubbing` turn it on and off.

## 5. Dual timeline (pp. 643–646, 689–692, 716–718)

**Upper timeline:**
- Always shows the whole program at full width; it has no zoom.
- Its playhead moves freely, and dragging its ruler scrubs the whole program.
- Most edits work there too, and clips can be dragged between the two timelines.
- It shows the timeline In/Out range and markers.

**Lower timeline:**
- A detail view at a fixed zoom around the playhead (p. 644).
- **Fixed Playhead on (default):** the playhead stays at the center and the clips move under it. Dragging the ruler drags the clips. Clips don't snap to the fixed playhead (p. 717).
- **Fixed Playhead off (Timeline Options):**
  - the clips stay put and the playhead moves
  - the view pages when the playhead reaches an edge
  - clicking the ruler moves the playhead, and the playhead can be dragged

**Track display:**
- Combined video+audio tracks are numbered 1, 2, …; audio-only tracks A1, A2, … sit below them.
- A video clip shows a filmstrip: each picture is the frame under it, the first one the clip's first frame. Its audio shows as a waveform band along the bottom, enlarged when the track is enlarged (p. 647).
- Display toggles: clip names; durations (seconds:frames, bottom-left); status icons.
- **States:**
  - locked track: clips are stippled
  - disabled track: clips are dimmed
  - disabled clip: dull
  - muted clip: a mute icon

**Track headers** (pp. 647, 678, 717): Enlarge (one track at a time) left of the name; Lock, Mute (audio) and Video Enable as icon columns on the right (D43).

**Automatic tools:**
- **Boring Detector** (Timeline Options, p. 690): on the upper timeline, highlights clips longer than "Boring Clips" (light gray) and shorter than "Jump Cuts" (red), live as you edit. Analyze Timeline Edits sets both (D31).
- **Detect Scene Cuts** (Timeline Actions, p. 690): splits the selected clips where their shots change (D30). Resolve has it in Studio only; vidcut has it in every build.

## 6. Track model (pp. 647, 649, 716, 721, 727)

- **Track 1 is the main story (A-roll):**
  - insert, delete, move and trim on Track 1 ripple, so it never has gaps
  - with **Ripple On** turned off, it behaves like the other tracks
  - holding Alt while trimming turns rippling off for that trim
- **Tracks 2+ are B-roll:**
  - they never ripple themselves; clips sit freely, with gaps
  - each clip on a higher track is attached to the Track 1 clip under its In point, and follows it when Track 1 ripples (p. 649; D21)
  - trimming that Track 1 clip's Out to before the attached clip's In deletes the attached clip (p. 727)
  - higher tracks ripple only where there is no Track 1 video under them (p. 721; D22)
- **Audio tracks:** A1, A2, … sit below the video tracks and hold sound alone, such as music. Media without video go there by default (D26).
- **Track header** (p. 647): the target-track toggle (D3), lock, mute and video on/off. The target track's header and row are lighter, the header with an accent edge. Right-click adds a video or audio track, or removes one (D27).
- **Overlapping video:** the highest enabled track is visible (p. 647). A disabled clip, or a track with its video off, shows what is under it.
- **Sound:** every audible track plays, mixed; mute and solo (§9) decide which are audible.
- **Linking:** a clip's video and audio are one combined clip. Trims move both, except that a J/L cut moves only the audio edit (§8.7). The Cut page has no link/unlink.

## 7. Edit functions (pp. 696–701)

**Workflow** (p. 696):
1. Pick a source.
2. Mark source In/Out.
3. Optionally pick the target track by clicking its header or number.
4. Press an edit button or key.
5. Press Q to review the result, and Q again to go back.

Video Only and Audio Only limit what an edit takes (p. 694).

### 7.1 Smart Indicator (p. 696)
- **What it marks:** a marker on the ruler shows the edit point nearest the playhead. Some functions use that point instead of the exact playhead frame.
- **How it looks:** in Resolve, after idling it bobs, and the chosen edit flashes green, or red if the clip there has no media left beyond its edge. vidcut shows the colors without the motion (deviation X15).

### 7.2 Edit functions
- **Smart Insert** (F9 on the Edit page; p. 697):
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
    3. Within 5 frames of an edit, with no marks, it lines up with that edit.
    4. Otherwise it lines up with the playhead.
- **Close Up, Source Overwrite (out).**

### 7.3 Drag and drop from the pool or Viewer (pp. 648, 679, 694–695)
- **Onto an empty timeline, or left of the first clip:** becomes the first clip.
- **Past either end of all clips:** appended or prepended.
- **Onto a clip, released right away** (the whole clip highlights): Ripple Overwrite replaces that clip.
- **Onto a clip after hovering briefly** (the highlight narrows to the incoming footprint): a plain Overwrite with no ripple, which may split the target.
- **Onto an edit point:** inserts at that edit.
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
- **Onto an edit point** (it highlights): inserts at that edit. The gap left behind closes, and later clips (with their attached higher-track clips) shift. Works with several clips.
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
- Join Match Edit Clips rejoins the halves of a split clip (later).

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
  - vidcut shows the outgoing clip's last frame beside the incoming clip's first frame, with the ±1 buttons. Draggable filmstrips: (later).
- **Trim Start/End to Playhead** (Edit Actions, p. 729): moves a clip's head or tail to the playhead.
- **Change Clip Duration** (Ctrl+D, p. 693):
  - enter frames or a timecode, or pick 1/5/15 seconds
  - End extends to the end of the media
  - "Extend Beyond Clip Length" pads with black (later)
  - applies to every selected clip
- **Dynamic Trim** (W, pp. 730–732): W toggles the mode, and the playhead turns yellow. J/K/L then trim instead of playing: the selected edit (Out, In or both, by U), or a slip or slide of the selected clips (right-click the Dynamic Trim button to choose). With nothing selected, it takes the edit nearest the playhead. Space plays around the selection; Up/Down move to another edit. Details: D32 and deviation X11.
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

## 9. Audio (pp. 642, 649–650, 743, 771, 773)

- **Meter:** green is safe, yellow is near the limit, red risks clipping.
- **Mixer** (pp. 649–650): the Mixer button opens a channel strip for each track with sound (name, meter, fader in dB, mute and solo), plus the mix's meter.
  - A dB scale stands beside each fader. Its numbers thin out when the fader is short; 0 dB is in the accent color, with a line across the fader.
  - Solo silences the tracks that aren't soloed. Mute and solo apply to export too (p. 650).
  - A fader is heard while it is dragged (`mixer.preview`, [preview.md](decisions/preview.md) §7) and saved on release, as one `track.set`.
  - Track pan and FX are out of scope.
- **Viewer meter and monitoring mute** (p. 642): a meter beside the viewer shows what it plays. Its speaker button mutes playback only, not the export.
- **Clip volume:** one gain value per clip, and clip mute.
- **Ripple Delete Silence** (p. 773): select clips, then Clip > Audio Operations > Ripple Delete Silence (in vidcut: the clip's right-click menu or Timeline Actions).
  - settings: threshold (dB), pre-head, post-tail, minimum to trip
  - previews the parts to remove as red stripes; Remove cuts them and closes the gaps, as one undo step
  - details and defaults: D29; the crossfade option: deviation X11

## 10. Quick Export (Ch. 39, pp. 788–790, 4095–4096, 4104–4109)

- **Flow:** File > Quick Export or the toolbar button, then pick a preset from the icon row, then Export, then choose a file name, then a progress dialog.
- **Range:** the timeline In–Out range if one is set, otherwise the whole timeline.
- **Output:** Quick Export always writes one file. Rendering individual clips is a Deliver-page feature in Resolve; our Export dialog covers it.
- **Preset models** (Deliver page):
  - H.264 Master: QuickTime, AAC 320 kb/s
  - H.265 Master: Main10
  - ProRes 422 HQ: LPCM audio
  - YouTube 720p–2160p: MP4, H.264 High
  - Vimeo
  - TikTok: vertical 1080×1920
- **User presets** can be added to the Quick Export row.
- **In vidcut:** the Quick Export row, the Export dialog and jobs: deviation X3. The suggested file name is the project's name. Timeline interchange: deviation X9.

## 11. Keyboard shortcuts (Linux and Windows)

Keys from the Cut chapters, with Edit-page keys filling the gaps (marked †).

Shortcuts do nothing while a text field has focus or a question (a modal dialog) is up; nothing behind a question sees the keys. Esc dismisses a question without acting: Cancel, or Keep Running on the quit question.

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
| Delete / Backspace | Delete selection (ripples on Track 1) | 725 |
| Alt+Delete | Lift the selection (leave the gap; D9) | 725 |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / cut (ripple) / paste at playhead | 723 |
| Ctrl+A / Ctrl+Shift+A | Select all / deselect all | 719 |
| Alt+Shift+V | Select clips under the playhead | 719 |
| V / U | Select nearest edit / cycle edit side | 720 |
| , / . (Shift: 1 s) | Trim the selected edit one frame left / right | 870† |
| D | Disable / enable clip video | 724 |
| Ctrl+D | Change clip duration | 693 |
| N | Snapping on/off | 716 |
| M / Alt+M | Add-or-edit marker / delete marker at playhead | 718 |
| W | Dynamic Trim on/off (then J/K/L trim, Space plays around) | 731 |
| Shift+S | Audio scrubbing on/off | 876 |
| Ctrl+mouse wheel | Zoom the lower timeline | 644 |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo | 651 |
| Ctrl+I | Import media | 654 |
| Shift+Ctrl+N | New bin | 655 |
| Alt+F / Shift+F / Esc | Enhanced viewer / full screen / exit (later) | 688, 40† |

## 12. Decisions on gaps and contradictions

A decision holds until the manual or a test shows that Resolve does otherwise; then change the decision and the code.

- **D1. Fixed playhead.** The lower timeline's playhead is fixed by default, with a Timeline Options toggle. The manual calls it fixed on p. 689 but describes a toggle on pp. 645 and 716.
- **D2. What Split cuts.** If some selected clips are under the playhead, only those are split. Otherwise every clip under the playhead on every unlocked track is split. Afterwards the left halves are selected. This simplifies the Edit page's Auto Select rule (p. 950).
- **D3. Target track.** Smart Insert, Ripple Overwrite and Overwrite use the selected target track (V1 by default; A1 for sound alone, D26). Append always uses Track 1; Place On Top picks its own track (D24). The manual doesn't say which track Append uses.
- **D4. Trim Start/End to Playhead.** Applies to the selected clips; with no selection, to the Track 1 clip under the playhead.
- **D5. Missing source marks.**
  - No source In means the clip's first frame; no source Out means its last frame.
  - In Source Tape mode with no marks, the edit takes the whole clip under the source-tape playhead.
- **D6. Marks persist after edits.** Source marks are remembered per clip, and timeline marks stay until cleared (Alt+X). This is the least surprising behavior for scripting.
- **D7. Overwrite with both timeline In and Out set.** A three-point edit: the timeline range sets the duration, starting from the source In. If there isn't enough source media, the edit fails with an error.
- **D8. Ripple Overwrite with both timeline In and Out set.** The source duration wins, and the program ripples by the difference (p. 698).
- **D9. Lift on Track 1.**
  - With Ripple On off, Delete leaves a gap. Alt+Delete lifts (leaves the gap) for one delete, as Alt suspends rippling while trimming.
  - The API: `edit.delete --lift`.
- **D10. Overwrite past the end of Track 1.** With Ripple On, an Overwrite that would start after the end of Track 1 starts at the end instead, so Track 1 stays gapless.
- **D11. Hover time.** 0.8 s of hovering turns a drop or move onto a clip from "replace" into "overwrite". The manual gives no value.
- **D12. Clip gain range.** −60 dB to +24 dB, plus mute. The manual gives no range.
- **D13. Keys.** Shortcuts the Cut chapters don't give come from the Edit page (marked † in §11). Mac to Linux and Windows: Command → Ctrl, Option → Alt.
- **D14. J/L cuts belong to an edit point.** The manual doesn't cover this. Without these rules, audio could overlap on a track or reach into gaps.
  - A J/L cut (§8.7) is stored as matching audio offsets on the two clips that meet there: the outgoing clip's `tail` equals the incoming clip's `head`.
  - When an edit gives a clip a different neighbor at that point, the J/L cut resets to a straight cut. Examples: a delete, a move, or a split inside the overlap zone (one half would otherwise have no audio of its own).
  - A clip can be heard only past its picture: its neighbor's J/L cut covers all of it, and its own sound runs on in its J/L cut at the other end. If that cut resets, the clip would have no audio left, so the cut at its other end resets too.
- **D15. Playhead after an edit function.** The manual doesn't say; this is the Edit page's behavior.
  - After Smart Insert, Append, Overwrite, Ripple Overwrite or Paste, the timeline playhead moves to the first frame after the new clips, ready for the next edit. The same requests from scripts and agents move it too.
  - The Viewer keeps its mode, so Q reviews the result (p. 696).
  - A clip dragged onto the timeline leaves the playhead where it was: a drop isn't an edit function.
- **D16. Mark Out includes the frame under the playhead.** As in Resolve, the Out frame is part of the range. vidcut stores ranges half-open, so pressing O at frame `f` stores `out = f + 1`. The Duration field and `--json` output use the stored values.
- **D17. Source Tape In/Out** (p. 692). In Source Tape mode, I/O mark the whole tape: the open bin's clips end to end, in pool order.
  - **Stored** as tape frames together with that clip order (`tape.marks.set` takes both), so an edit takes the same span whatever the pool shows later.
  - **Per-clip marks:** a span within one clip is also that clip's source marks (D5), used by pool thumbnails and Source Clip mode. A span across clips is only tape marks. Clearing a one-clip span clears that clip's marks too; clearing a crossing span leaves per-clip marks alone.
  - **Edited:** an edit function given `tape: true` instead of `media` takes the span: the tail of the first clip, the clips in between, and the head of the last. They go in end to end as one block, by the same rules as one clip:
    - an insert moves what follows by the whole span
    - Ripple Overwrite replaces the clip (or the marked range) with all of it
    - Place On Top aligns the block once and puts it on one track
    - timeline In and Out (D7) fit the block as a whole
    - it is one undo step
  - **Cleared:** removing one of the tape's clips, or relinking one to a file of another length, clears the tape marks, since their frames would point elsewhere.
- **D18. Drop and move targets.**
  - Within 6 px of an edit point, a drop inserts there; the moved clips' own edges don't count.
  - On another clip: replace if released within 0.8 s of reaching it (D11), else overwrite at the block's position. Several clips always overwrite (Replace takes one clip).
  - Past the last clip: the clips move (or the media is appended) to the end. Left of the first clip: inserted at the start.
  - The row under the pointer is the destination track. The strip above the top row means a new video track. The bottom row sits on the timeline's lower edge; up to 24 px below it means a new audio track, shown there while the drag lasts.
  - Dragged off the tracks (more than 24 px to either side, above the top strip or further below), or Esc during the drag: cancelled, nothing changes.
- **D19. Trimming with the pointer.**
  - Edges follow the pointer's movement from the press (they don't jump to it), snapped to targets (§8.8).
  - A trim, roll, slip or J/L cut stops where the media runs out instead of failing.
  - Safe Edit (p. 732) holds an edge at the neighbor for 0.6 s; pushing on overrides it.
  - Dragging empty space scrubs in the upper timeline and draws a marquee in the lower one.
- **D20. Keyboard trimming.** With an edit selected (V, U, or a click on an edit), `,` and `.` trim it by one frame, and Shift+`,`/`.` by one second: the Out, the In, or both (a roll). These are the Edit page's nudge keys (†); the Cut page's own keyboard trimming is Dynamic Trim (D32).
  - **The J/L handle is the audio edit.** Clicking it (§8.7) selects that audio edit on the clip's track (D36). The selection's frame is the audio edit, `tail` frames after the picture cut.
  - `,` and `.` then move the outgoing clip's `tail` and the incoming clip's `head` together (`edit.audio_roll`). The picture cut stays where it is.
  - U cycles Out, In and both on a picture edit, and leaves an audio edit selected.
- **D21. Following Track 1** (pp. 649, 727). A clip on another track is attached to the Track 1 picture under its first frame: that clip and source frame. Nothing is stored; each edit works it out from the timeline before and after.
  - When that picture moves (later clips shift, or its clip moves, even to another track), the attached clip moves with it. Split parts count as the same picture.
  - When its clip's tail is trimmed past it, it is deleted (p. 727). When its clip's head is trimmed past it, it moves to the clip's new start.
  - When its clip is replaced (Ripple Overwrite, Replace), it stays where it is.
  - When its clip is deleted, it moves to where Track 1 closed up.
  - Over a gap or past the end of Track 1, it keeps its distance to the next Track 1 clip (or from the end of the one before).
  - Clips on locked tracks don't follow. A clip that would land on another clip moves up to the first free unlocked track of its kind (audio tracks: down), created if needed.
- **D22. Rippling other tracks** (p. 721). With Ripple On, deleting or trimming a clip on another track ripples that track only when Track 1 has no clips from that clip's start on; otherwise it leaves a gap. Deleting a timeline In–Out range takes it out of every unlocked track and closes them up together (p. 725).
- **D23. Edit functions on other tracks.** Only Track 1 pushes clips (p. 697). On another track, Smart Insert and insert overwrite, and Ripple Overwrite replaces without moving anything else. Paste puts each clip back on its own track (p. 723), creating missing tracks. Split follows D2 on every unlocked track.
- **D24. Where Place On Top goes** (pp. 699–700). Onto the video track above the highest one with clips in the incoming span, created if needed; onto Track 1 only if nothing is there (then D10 applies). Sound alone goes to the first audio track with room, created if needed.
- **D25. Moving clips between tracks.**
  - Moves are given in timeline positions from before the move.
  - A Track 1 clip moved to another track lifts out. With Ripple On, Track 1 closes up and the moved clips land where they were dropped.
  - Clips moved onto Track 1 insert or overwrite there (§8.2).
  - Video can't go onto an audio track. A selection spanning several tracks moves along its tracks only.
- **D26. Sound alone.** An edit without picture (media without video, or the Audio Only toggle, p. 694) goes to A1 unless a target track is given. A clip on an audio track keeps only its audio. In the GUI, sound alone dragged onto a video track lands on the first audio track.
- **D27. Track numbers are positions.** Video tracks are V1, V2, … from the bottom; audio tracks A1, A2, … from the top. Removing a track renumbers the ones after it. Edits and moves may name the next track (one past the last) to create it. Track 1 can't be removed, and removing a track with clips must be confirmed (`--force`). Track volume runs from −60 dB to +24 dB, like clip gain (D12).
- **D28. Meters.** Each meter has one bar per channel of the project's channel count (`settings.channels`; usually stereo), as on p. 649. Peaks are linear, drawn from −60 dBFS to 0, and each bar holds its peak for 1.5 s.
  - **Levels:** the bars read the media waveforms at the playhead, one peak per channel per 10 ms, whether playing, paused or scrubbing, with clip gain and the track fader applied. A muted clip is silent. Mute and solo decide which tracks are heard in the mix and which strips show silence, as they do for export (p. 650). The mix sums the audible tracks per channel, as the preview does.
  - The meters show the edit's levels, not the player's output, so they move while paused too ([todo](todo/meters-tap-player-output.md)). The viewer's monitoring mute (`player.mute`) silences the viewer meter only.
  - **Channels:** each waveform is mixed to the project's channels the way the export mixes it: a 5.1 source's centre and surrounds feed a stereo meter's left and right, and a mono source lights every bar. The mix can pass full scale when tracks add up.
- **D29. Ripple Delete Silence** (p. 773). The manual gives no defaults or measuring rule.
  - **Silence** is where every channel's peak, after the clip's gain, stays below the threshold, measured in 10 ms steps. Parts of the clip past the end of its audio count as silent.
  - **Defaults:** threshold −40 dBFS, pre-head and post-tail 0.15 s, minimum to trip 0.5 s. They are in seconds, not frames, so the project's frame rate doesn't change what they mean.
  - **What goes:** each silence at least the minimum long, less the post-tail after the sound before it and the pre-head before the sound after it. Silence at a clip's start or end goes entirely. Only whole frames are cut.
  - **One edit:** it ripples whatever Ripple On says, as its name promises. Track 1 closes up, and clips on other tracks follow (D21). It is one undo step; a dry run reports the parts without cutting (the red stripes).
- **D30. Finding shots and black.** Detect Scene Cuts (p. 690) gives no algorithm.
  - **Shots:** each frame gets FFmpeg scdet's scene score on its luma: the mean difference from the previous frame (0–100), or the change in that difference if smaller, so steady motion doesn't read as a cut. A frame scoring at least 10 starts a new shot. The cut lands on the first timeline frame that shows it.
  - **Black:** a frame is black when 98% of its pixels are within 10% of black. Black stretches of 0.5 s or more are reported (`analyze.black`).
  - **Measurement:** pictures are measured at 96×54 and sound in 10 ms peaks. Measurements are cached per file, so trying other thresholds is quick.
- **D31. Boring Detector defaults** (p. 690). The manual gives none. Clips over 30 s are boring, and clips under 10 frames are jump cuts (2 frames finds flash frames, p. 690). The detector looks at clips on video tracks. Its settings are view options kept between runs, not part of the project.
- **D32. Dynamic Trim** (pp. 730–732).
  - **Running:** L or J runs the trim at the shuttle speed (repeat for 2×, 4×, 8×), as playback would move. The run stops where the media or a neighbor runs out, and shows it: the selected cut turns red and the viewer says why.
  - **Seeing it:** the viewer shows the two-up: the frames on both sides of the edit, or of the slipped or sliding clip, as the trim leaves them (exact frames, as in the Trim Editor). The timeline draws the clips as the trim leaves them, so the cut moves during the run.
  - **The selection:** the selected edit follows the trim. An undo, or someone else's edit, that removes the selected cut clears the selection.
  - **Applying it:** K stops the run and applies it as one request (`edit.trim`, `edit.roll`, `edit.slip`, `edit.slide`, or `edit.audio_roll` when the selected edit is the J/L handle). K held with a tap of J or L applies one frame. W also applies anything pending before leaving the mode.
  - **Play around:** Space plays from 2 s before the selection to 2 s after it.
- **D33. Rotation metadata.** The manual doesn't say. Media show as players show them: the rotation in a file's display matrix (phones store portrait video sideways with one) is applied in the preview, export, frames, thumbnails and storyboards.
  - A portrait clip that sets the project format makes a portrait project.
  - `media.get` reports the size as shown, and the rotation.
  - Mirrored display matrices are taken as rotations only.
- **D34. Interlaced sources** (Deinterlace Quality, p. 741).
  - **Detection:** a stream is interlaced if its field order says so, or if its first frame is flagged interlaced. `media.get` reports the field shown first as `fields`.
  - **Deinterlacing:** frames flagged interlaced are deinterlaced everywhere (export, frames, storyboards, the preview), one progressive frame per frame. The first field (in time) is kept, and the second is rebuilt from it and the neighboring frames (`bwdif`). There is one method, as the Cut page's playback always uses Normal quality (deviation X13).
  - **Rates:** output is never interlaced. An interlaced clip in a project at twice its frame rate shows each frame twice (no field-rate output).
- **D35. HDR and color** (Resolve's color management, pp. 230–239).
  - **Detection:** a clip is HDR when its transfer is PQ (SMPTE ST 2084) or HLG; `media.get` reports it as `hdr`.
  - **SDR output:** HDR clips are tone-mapped to BT.709. Up to 80% of the 203-nit reference white stays as it is; brighter highlights roll off, up to 1000 nits. Which output an export gets: D44.
  - **HDR output:** PQ or HLG (`video.hdr`, chosen by D44 or set) writes BT.2100 in 10 bits. SDR clips are placed with their white at 203 nits (BT.2408), like Resolve's "203 nits" option (p. 238).
  - **Tags:** outputs are tagged with their colors. Untagged media are taken as BT.709 when 1280 wide or over 576 lines, else BT.601, as players commonly guess; the viewer reads them the same way (a 1280×534 widescreen film is HD).
  - **Preview:** the viewer's shader converts and tone-maps with the export's math, so HDR clips look as in an SDR export (deviation X14).
- **D36. Which track an edit point is on** (pp. 696, 720, 729–732). The manual pictures one main story.
  - **Track 1:** the Smart Indicator, V (select the nearest edit), Smart Insert, Place On Top's alignment and Up/Down use Track 1's edits.
  - **A clicked edit keeps its track:** clicking a clip's edge, a cut, or the J/L handle on a cut selects an edit on that track. The handle selects the audio edit (D20); an edge or a cut selects the picture edit.
  - `,`/`.`, U, the Trim Editor and Dynamic Trim then work on that track. The Trim Editor's labels and frames are that track's clips, whatever covers them. Dynamic Trim's Up/Down move along that track's edits, and an audio selection stays on the audio edit of the cut they land on.
  - **During Dynamic Trim**, `,` and `.` do nothing while it runs; its pending trim is the one request K sends (D32).
- **D37. The project format: chosen at the start, or set by the first video without asking** (§3; Resolve offers to match the first import, p. 654).
  - **Set by the first video:** in a project created without a format, the first media file with video sets the frame rate and the picture size as shown (D33). There is no prompt: a dialog would stop scripts and agents.
  - **Chosen:** New Project (and `project.create`, or `project.open` with `create`) takes a picture size and a frame rate. The size is any even width and height from 16 px. The rate is any positive rate, as `num/den` or a decimal (12.5, 29.97, 30000/1001), not only the common ones the list offers (Resolve allows a fixed set).
  - **Changed later:** File > Project Settings (`project.set`) changes the size at any time, since every clip is fitted to the frame when it is drawn. The frame rate changes only while the timeline is empty, because timeline positions are whole frames at that rate.
- **D38. Stills** (§3; pp. 114, 1194). The manual gives two lengths for a still: the standard still duration, 5 s by default (p. 114), and "10 seconds" for imported stills (p. 1194). vidcut follows the preference's 5 s.
  - A still edited without a range or marks lasts the project's still duration (`project.set still_duration`, deviation X16). The Viewer shows it as a clip that long.
  - A range or a trim can make its clips longer, up to 17 h 40 min (p. 1194). Every frame shows the one picture, so a still has no frame rate, and the first video still sets the project format (D37).
  - Media are images when FFmpeg reads them as one (its `image2` and `*_pipe` demuxers).
- **D39. Image sequences** (§3; p. 366). Numbered files (`frame_000001.png`, `frame_000002.png`, …) come in as one clip with `media.add sequence`: the files numbered in a row around each file given, with no number missing. Its frame rate is the project's unless `fps` says otherwise (Resolve takes it from Clip Attributes).
  - Numbers zero-padded to a width (`%06d`) or not (`%d`) both work; the width shows in the file names.
  - The pool names it as Resolve does: `frame_[000001-000120].png`.
  - Without `sequence`, a numbered image is a still (deviation X17).
- **D40. Timeline interchange with Resolve** (deviation X9; pp. 527–530, 4160). The manual describes Resolve's OTIO import and export, not the files. These rules follow the files Resolve exports and what its import needs.
  - **Source positions are timecode.** A source range counts from the media file's start timecode (a `tmcd` track, MXF's, a broadcast WAV's time reference), as Resolve writes and reads it; the available range starts there too. Media without one start at 0. Drop-frame timecode is converted with its skipped numbers left out.
  - **Paths are plain.** Resolve reads `target_url` as a path: `file://` URLs and `%20` escapes don't find the file. vidcut writes plain absolute paths (relative ones in bundles), and reads URLs too, escaped or not.
  - **Audio stays with its video** through Resolve's link group number, written on both and read when present.
  - **Markers:** notes are written in `Resolve_OTIO.Note` as well as `comment`. A marker keeps its length (one frame unless given one; `markers.add` and `markers.set` take `duration`), carried as OTIO's marked range. The timeline draws it as a point.
  - **Names:** track names are deviation X9 (a name that was set is written and read back; the place defaults `Video 1` and `Audio 2` don't count as names). Resolve's export writes no timeline name, and its import doesn't keep one.
  - **Bundles:** `.otioz` as OpenTimelineIO's adapter writes it (what Resolve imports); `.otiod` is the same as a folder. A bundle unpacks into a folder named after it, as Resolve's import does (deviation X9).
- **D41. Proxies** (deviation X18). The manual's proxy chapters (pp. 207–218) are outside the Cut page, so the playback rules are set here.
  - **What gets one:** videos and image sequences, on demand (`proxy.generate`; Media Pool: Generate Proxy). Stills and audio-only media don't: naming one is an error, and generating with no ids skips them.
  - **Codec:** the preferences' `proxies.codec`:
    - `auto` (the default): AV1 on a hardware encoder (NVENC, then VAAPI) when one works here, else H.264 with x264, which stays quick on old computers.
    - `av1`: AV1, with SVT-AV1 when there's no hardware for it.
    - `h264`: x264.
    - Every proxy is 8-bit 4:2:0, with a closed GOP of 60 frames, each frame coded in the order it shows. The codec is part of the file name.
  - **Size:** the longest side is the preferences' `proxies.size` (960 pixels unless changed), even, from 16 to 7680; `width` overrides it. The aspect ratio is kept, and a smaller picture keeps its size.
  - **Files** go in `proxies/` beside the project, or `~/.cache/vidcut/proxies` when the project's folder is unknown.
  - **Automatic:** with `proxies.auto` (on by default) and `use_proxies` on, the running GUI makes proxies by itself for:
    - image sequences (`proxies.image_sequences`)
    - videos with more pixels than `proxies.larger_than` (3840×2160: a GPU plays 4K)
    - video files averaging more than `proxies.above_mbps` (150 Mb/s: camera and editing formats a GPU can't decode)

    It checks the media an import adds, and the whole pool when `use_proxies` is turned on; never otherwise, so a proxy someone removed stays removed until then. It sends the session's own `proxy.generate` with `auto: true` and the media named (origin `session` in the log), a job like any proxy encode; media already being encoded are left out. Headless runs never start encodes by themselves: scripts and agents don't need proxies. `auto: true` on any `proxy.generate` (`vidcut proxy generate --auto`) takes only media the rules pick that have no fresh proxy; finding none is not an error.
  - **Encoding** runs beside the session, like an export. Progress is shown. Cancelling (Ctrl-C, or the client leaving) stops it and leaves no file. Proxies are recorded on the project once their files are written. The GUI keeps working meanwhile; a headless run takes the project's lock only to record them. In a running GUI the encode is a job ([gui.md](decisions/gui.md) §9): the Jobs window, which Generate Proxy opens, shows its progress and cancels it, as does `jobs.cancel`.
  - **Switch:** `use_proxies` (saved in the project; the Media Pool's Proxies checkbox) makes the preview play proxies. Export, frame render, analysis and OTIO always use the originals.
  - **Fresh:** a proxy is used only when its file is present and its stored source size and modification time match the pool item; otherwise the original plays. Relinking to the same file keeps the proxy; relinking to another file drops it. An image sequence's size and time cover the whole run: the total of its pictures' sizes and the newest of their times, read once when it is added or relinked, so a render that rewrites later pictures counts as a changed file. Nothing is read from disk when playback starts.
  - **Frame exact:** proxy frames carry the source's presentation timestamps. The preview keeps `origin` and `source_frame` and replaces only the video path, so a sequence plays as that video and the export's sampling rule picks the same frames. Preview audio stays on the original.
- **D42. Audio scrubbing** (§4.5; p. 876). The manual says only that dragging the playhead plays the sound when scrubbing is on, as it is by default.
  - **What plays:** a frame step (←/→, K held with J/L tapped) and every move of a paused playhead (a drag is many moves) play the sound of the frame arrived at: its samples of the mix, every audible track with its gains and fader, faded in and out over 3 ms so it doesn't click. The monitoring mute silences it. During playback nothing extra plays.
  - **Always forward:** a step back plays its frame forward too. A frame is 33–42 ms: reversed, it isn't heard as backward, and played forward the start of a word stays recognizable.
  - **One at a time:** a frame's sound plays whole. A newer one waits for it and replaces any still waiting, so a held key plays frame after frame and nothing piles up.
  - **Playing backward** (J) plays the sound reversed, keeping its pitch at other speeds as forward play does.
- **D43. Enlarging a track** (pp. 647, 678). The manual says a track's sizing button enlarges it, showing its pictures and sound full size, one track at a time.
  - **Size:** the enlarged track is three times its usual height, and at least 180 px. In a timeline too short for that, the other tracks shrink first, down to one line each, and the enlarged track takes what they leave.
  - **A view, not an edit:** like the zoom, it is how the lower timeline shows the tracks: no request, nothing in the project or the undo history, and it ends with the window. The upper timeline doesn't change.
  - **Buttons:** the header's buttons are icons that show the state: a lock closed or open; a speaker and an eye crossed out when off. A button is lit when its track is out of the ordinary: locked, muted, video off, enlarged.
- **D44. An export's dynamic range and bit depth follow its footage** (§10; deviation X14). In Resolve, exports deliver what the project's color management is set to. vidcut has none, so each export decides from the pictures it shows, unless told.
  - **What counts:** the frames the export shows from sources: the visible top clip at each frame of the range, weighted by duration. Gaps (black) don't count. A per-clip export decides for each file from its own clip.
  - **Dynamic range** (`video.hdr`, default `auto`): HDR when more than half of that picture is HDR and the codec carries HDR (HEVC, AV1, VP9) with an encoder here that writes 10 bits. It is HLG when every HDR frame is HLG, otherwise PQ. Anything else is SDR, with the HDR frames tone-mapped (D35). `sdr`, `pq` and `hlg` set it.
  - **Bit depth** (`video.bit_depth` unset):
    - HDR: 10.
    - SDR: 10 when more than half of the picture has 10 bits or more and the codec's 10-bit form plays widely (HEVC Main10, AV1, VP9 profile 2). H.264 stays 8: High 10 plays in few places.
    - ProRes is 10-bit. DNxHR has its profile's depth, and DNxHR HQ becomes HQX, its 10-bit form at the same data rate.
    - SDR gets 10 bits only from the encoder an 8-bit export would start with (a GPU's, for `auto`). If that encoder can't write them, the export stays 8-bit rather than switch to a slower encoder.
    - A set depth, or a profile that fixes one, wins.
  - **Presets:** the masters (H.265, AV1, VP9) and the H.264 presets use `auto`, so H.264 comes out SDR, with the note below. TikTok asks for `sdr`, since it is watched on phones.
  - **Reporting:** the plan and the result give each file's choice, what it came from (frames of PQ, HLG and 10-bit picture) and why. When HDR frames will be tone-mapped and SDR wasn't asked for, the plan adds a note with the overrides that keep them HDR (HEVC when the codec can't carry it). The Export dialog shows the note as one dim line under the plan, with an Export HDR button that applies them. It never asks: Quick Export and exports go on as they are. With `sdr` set, nothing is said.
- **D45. Passes** (§10; Resolve's multi-pass option is on the Deliver page, outside the Cut page). Two passes bring an export closer to a target bitrate and spread the bits better; at constant quality they aren't worth the time.
  - **`video.passes`:** 1 or 2. Unset: 2 with a target bitrate when the encoder makes two, else 1. Two need a bitrate.
  - **Who makes two:** x264, x265 and VP9 encode the range twice, the first time only to measure. NVENC looks at each frame twice within one encode. VAAPI, AMF, QSV, ProRes, DNxHR and SVT-AV1 make one ([todo](todo/svt-av1-two-passes.md)); asking them for two is an error, and `auto` moves on to an encoder that makes two.
  - **Settings and defaults:**
    - `first_pass`: `fast` (quicker settings for the measuring run) or `full`.
    - `multipass`: `quarter` (NVENC's first look at a quarter of the resolution) or `full`.
    - `lookahead`: 32 frames on NVENC whatever the rate control (0 to 32; 0 for none).
  - **While it runs:** progress says which run it is. Cancelling either run leaves nothing. The statistics, in a hidden folder beside the output, are removed however it ends. A per-clip export makes both passes for each file. The Export dialog shows the passes for a bitrate, with the first-pass speed or NVENC's settings for the encoder the plan names.
- **D46. HDR metadata follows the footage** (§10; deviation X19). In Resolve, HDR exports carry what the grading setup says (the Mastering Display, Export/Embed HDR10 Metadata, p. 4117). vidcut doesn't grade, so it carries what the sources say.
  - **Read** when media are added: the mastering display (SMPTE ST 2086), the content light levels (MaxCLL, MaxFALL), whether frames have HDR10+ (ST 2094-40), and the stream's Dolby Vision profile. HDR media whose entry has none of these are probed again at export.
  - **Static values** (`video.hdr_metadata`, default `auto`; PQ output only): the widest color volume of the pictures shown (primaries that hold all of theirs, the highest peak, the lowest black) and their highest MaxCLL and MaxFALL.
    - SDR pictures count as BT.709 at 203 cd/m², with light levels of 203.
    - PQ pictures without a mastering display count as BT.2020 at 1000 cd/m², the peak vidcut's tone mapping assumes.
    - HLG pictures count as BT.2020 at 1000 cd/m², HLG's reference display.
    - `video.mastering_display` and `video.max_cll` (in x265's syntax) set them.
    - HLG and SDR output carry none.
  - **Measured light levels:** a picture without its own (a PQ file without them, HLG converted into PQ) is measured in the frames the export shows, as the export converts them. MaxCLL is the brightest channel of the brightest pixel; MaxFALL is the highest frame average of each pixel's brightest channel. The averages, and the percentiles HDR10+ is made with, use at most half a million pixels of a frame, a close estimate.
    - `video.max_cll=measure` measures every picture instead of reading the files.
    - Measuring happens when the export starts, before encoding, and progress says so; a plan says the levels will be measured then.
    - Only what the export shows of a file is measured. The result is kept, so the next plan or export of the same footage has it at once.
  - **Dynamic metadata:** HDR10+ and Dolby Vision describe each frame's picture, so each output frame keeps those of the frame it shows, where its transfer stays the same.
    - HDR10+ (PQ output only) is written when any picture shown has it. Frames without it get HDR10+ made from their measured light (each channel's peak, the average and percentiles; no tone-mapping curve); black frames get that of no light.
    - Dolby Vision is written when every picture shown has it, with one profile: 8.1 into PQ, 8.4 into HLG (10 for AV1). An encoder writing it needs it on every frame, so black frames take the Dolby Vision of the frame before them, and black before the first picture goes without. libx265 and libsvtav1 write it, so `auto` then encodes on the CPU.
    - `static`, `hdr10plus`, `dolby_vision`, `all` and `none` choose what is written. `hdr10plus` makes HDR10+ for every frame when no picture has its own.
  - **Where:** in the bitstream (HEVC SEI at every keyframe, AV1 metadata OBUs; HDR10+ on every frame) and in the container (MP4/MOV `mdcv`/`clli`, Matroska/WebM colour metadata, Dolby Vision's configuration; VP9's HDR10+ as WebM BlockAdditions).
  - **Reporting:** the plan and the result give each file's metadata and why (what was left out, and for what). The Export dialog shows it in one dim line, and offers the setting next to the dynamic range.
- **D47. Drop-frame timecode** (§4.3; Resolve's project settings offer it at these rates). At 29.97 and 59.94 fps a project can count record timecode drop-frame: frame numbers 00 and 01 (00–03 at 59.94) are skipped at the start of every minute except every tenth. So `01:00:00;00` is an hour of clock time, frame 107892; non-drop `01:00:00:00` is frame 108000.
  - **Setting:** `project.set drop_frame` (`--drop-frame true`), and `project.create` and `project.open`'s create. New Project and File > Project Settings show it at those rates. It is refused at other rates, and changing to a rate without it turns it off.
  - **Shown** with `;` before the frames wherever timecode appears: the viewer, the ruler, storyboards, query results' `*_tc` fields, lengths and source positions. A media file's own timecode shows as that file counts it.
  - **Typed:** `;` means drop-frame anywhere (`01:00:00;00`, `01;00;00;00`); in a drop-frame project `:` counts drop-frame too. A label that drop-frame skips (`00:01:00;00`) is refused.
  - **Set automatically:** the first video of a project whose format it sets (D37), and an OTIO import into an empty timeline, turn it on when the media's own timecode is drop-frame (written with `;`). OTIO doesn't say whether a timeline is drop-frame; only frame numbers travel.
