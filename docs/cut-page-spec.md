# Cut page behavioral spec

vidcut copies the behavior of DaVinci Resolve's **Cut page** by default. This document describes that behavior in our own words. Each item carries a page reference to the *DaVinci Resolve 21.1 Reference Manual* (Chapters 33–39, pp. 635–790). Printed page numbers equal PDF page numbers. Deviations are listed in [deviations.md](deviations.md); the Decisions section at the end settles points the manual leaves open.

**Reading the manual:** it is 183 MB, so extract text with `pdftotext -f 635 -l 790 <manual>.pdf -`. The developer's copy lives in `$VIDCUT_TEST_DATA` (default `~/Downloads/vidcut-tmp`). Don't copy manual text into the repo; write behavior in your own words with page numbers.

**Key names:** the manual uses Mac keys. On Linux, Command → Ctrl and Option → Alt.

**Status markers:**
- **(v1)** planned for the single-track (V1) milestones M1–M5.
- **(M6)** waits for multi-track; **(M7)** belongs to the agent-extras milestone.
- **(later)** wanted, not yet scheduled.
- **(out)** out of scope.

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
  - **Upper timeline:** track-number buttons to its left select the target track; Audio Only and Video Only toggles sit there too (pp. 643, 694).

## 3. Media Pool (Ch. 34, pp. 653–675)

- **Import (v1)** (p. 654):
  - Ctrl+I, the Import Media button, or right-click > Import Media opens a file dialog with multi-select.
  - Import Media Folder imports a whole folder.
  - The first import into an empty project offers to change the project frame rate to match the media.
- **Remove (v1)** (p. 654): Delete or Backspace, after a confirmation. Only the pool entry goes; files on disk stay.
- **Views:**
  - **Thumbnail (v1)** (p. 664):
    - hovering scrubs the clip
    - I/O while hovering marks source In/Out
    - clicking the thumbnail's lower-right corner shows its metadata
  - **List (v1)** (p. 665): sortable columns.
  - **Filmstrip** and **Metadata** cards (later).
- **Sort and search (v1)** (pp. 666–667):
  - Sort Media By: Clip Name, Date Imported, Date Modified, Duration (ours), Timecode, ascending or descending.
  - The sort order is also the Source Tape order.
  - The search field filters live.
- **Bins (later)** (pp. 655–656): nested bins, a breadcrumb, Create Bin With Selected Clips. In v1 the pool is flat.
- **Clip color (later)** (pp. 667–668).
- **Interaction:**
  - **Click:** selects. In Source Clip mode the Viewer shows the clip; in Source Tape mode the playhead jumps to the clip's first frame (pp. 680, 704).
  - **Double-click:** opens the clip in the Viewer as Source Clip (p. 680).
  - **Drag onto a timeline:** a drag-and-drop edit (§7.3).
- **Selection** (p. 719): click, Shift-click range, Ctrl-click toggle, marquee, Ctrl+A.

## 4. Viewer (pp. 639–640, 679–693, 713)

### 4.1 Modes (v1)
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

### 4.2 Transport (v1) (pp. 640, 687–688, 870–872)
- **Play and step:**
  - Space: play/stop.
  - J/K/L: shuttle. Repeated J/L speeds up; K+J or K+L plays slowly; holding K and tapping J/L steps a frame (pp. 696, 871).
  - ←/→ step one frame; Shift+←/→ step one second (Edit-page keys, p. 870).
- **Jumping:**
  - Up/Down: previous/next edit point (p. 640).
  - Home/End: start/end; Shift+I / Shift+O: go to In / Out (p. 870).
- **Loop:** Ctrl+/ (later).
- **Jog control:** click and drag to scrub finely (later).

### 4.3 Timecode (v1) (pp. 640, 688, 713–714)
- **Go to a time:**
  - `=` then a timecode goes to that absolute time.
  - `+` or `-` then a value jumps relative to the current position.
  - Clicking the timecode field allows typing.
- **Display options** (right-click):
  - source timecode or source frame count (source modes)
  - record timecode or record frame (Timeline mode)
  - Timer 1/2 and KeyKode (later)

### 4.4 In/Out (v1) (pp. 691–693, 700)
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
- Audio scrubbing (Shift+S; on by default in Resolve, p. 876) (later).

## 5. Dual timeline (pp. 643–646, 689–692, 716–718)

**Upper timeline (v1):**
- Always shows the whole program at full width; it has no zoom.
- Its playhead moves freely, and dragging its ruler scrubs the whole program.
- Most edits work there too, and clips can be dragged between the two timelines.
- It shows the timeline In/Out range and markers.

**Lower timeline (v1):**
- A detail view at a fixed zoom around the playhead (p. 644).
- **Fixed Playhead on (default):** the playhead stays at the center and the clips move under it. Dragging the ruler drags the clips. Clips don't snap to the fixed playhead (p. 717).
- **Fixed Playhead off (Timeline Options):**
  - the clips stay put and the playhead moves
  - the view pages when the playhead reaches an edge
  - clicking the ruler moves the playhead, and the playhead can be dragged

**Track display:**
- Combined video+audio tracks are numbered 1, 2, …; audio-only tracks A1, A2, … sit below them.
- A video clip shows a filmstrip. Its audio shows as a waveform band along the bottom, which is enlarged when the track is enlarged (p. 647).
- Display toggles: clip names; durations (seconds:frames, bottom-left); status icons.
- **States:**
  - locked track: clips are stippled
  - disabled track: clips are dimmed
  - disabled clip: dull
  - muted clip: a mute icon

**Track headers:**
- Enlarge (one track at a time), Lock, Mute (audio), Video Enable (pp. 647, 717).

## 6. Track model (pp. 647, 649, 716, 721, 727)

- **Track 1 is the main story (A-roll):**
  - insert, delete, move and trim on Track 1 ripple, so it never has gaps
  - with **Ripple On** turned off it behaves like the other tracks
  - holding Alt while trimming turns rippling off temporarily
- **Tracks 2+ are B-roll (M6):**
  - they never ripple themselves, and clips sit freely with gaps
  - each higher-track clip is attached to the Track 1 clip under its In point and follows that clip when Track 1 ripples
  - trimming the anchor's Out to before the attached clip's In deletes the attached clip (p. 727)
  - higher tracks only ripple where there is no Track 1 video under them (p. 721)
- **Overlapping video:** the highest enabled track is visible (p. 647).
- **Linking:** video and audio of a clip are one combined clip. Trims move both, except that a J/L cut moves only the audio edit (§8.7). There is no link/unlink on the Cut page.

## 7. Edit functions (pp. 696–701)

**Workflow** (p. 696):
1. Pick a source.
2. Mark source In/Out.
3. Optionally pick the target track by clicking its header or number.
4. Press an edit button or key.
5. Press Q to review the result and Q again to go back.

Video Only and Audio Only limit what an edit takes (p. 694).

### 7.1 Smart Indicator (v1) (p. 696)
- **What it marks:** a marker on the ruler shows the edit point nearest the playhead. Some functions use that point instead of the exact playhead frame.
- **How it looks:** after idling it bobs, and the chosen edit flashes green. It flashes red if the clip there has no media left beyond its edge.

### 7.2 Edit functions
- **Smart Insert (v1)** (F9 in the Edit page; p. 697):
  - inserts at the Smart Indicator's edit point on the target track, never at an arbitrary frame
  - on Track 1, everything after that point moves right by the incoming duration
- **Append (v1)** (Shift+F12; p. 697): ignores the playhead and places the clip after the last clip.
- **Ripple Overwrite (v1)** (Shift+F10; p. 698):
  - with no timeline marks, it replaces the whole clip under the playhead
  - with a timeline In/Out, it replaces exactly that range, possibly across several clips
  - everything after shifts by (incoming − replaced) duration, in either direction
  - the source duration always wins; nothing is fitted to the gap
- **Overwrite (v1)** (F10; p. 701):
  - writes over the target track at the playhead, at the timeline In, or backtimed to a lone timeline Out
  - other clips don't move
- **Place On Top (M6)** (F12; pp. 699–700):
  - goes to the first track above every clip at that position, creating one if needed
  - ignores the selected track and never ripples
  - **Alignment priority:**
    1. A timeline In lines up with the source In.
    2. A lone timeline Out lines up with the source Out (backtimed).
    3. Within 5 frames of an edit with no marks, it lines up with that edit.
    4. Otherwise it lines up with the playhead.
- **Close Up, Source Overwrite (out).**

### 7.3 Drag and drop from the pool or Viewer (v1) (pp. 648, 679, 694–695)
- **Onto an empty timeline, or left of the first clip:** becomes the first clip.
- **Past either end of all clips:** appended or prepended.
- **Onto a clip, released right away** (the whole clip highlights): Ripple Overwrite replaces that clip.
- **Onto a clip after hovering briefly** (the highlight narrows to the incoming footprint): a plain Overwrite with no ripple, which may split the target.
- **Onto an edit point:** insert at that edit.
- **Into the gray area above the top track:** creates a new video+audio track (M6). Below the bottom track, an audio clip creates an audio-only track (M6).

## 8. Timeline editing (Ch. 36, pp. 715–732)

### 8.1 Selection (v1) (pp. 719–720)
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

### 8.2 Moving clips (v1 for Track 1; M6 for others) (pp. 720–723)
- **Onto another Track 1 clip, released quickly:** replaces it and ripples.
- **After hovering first:** overwrites the middle of the target, splitting it; the gap left behind closes on Track 1.
- **Onto an edit point** (it highlights): insert-at-edit. The gap left behind closes, and later clips (with their attached higher-track clips) shift. Works with several clips.
- **Track 1 clip dropped onto another track:** can only overwrite.
- **Higher-track clips:** move freely and leave gaps (M6).

### 8.3 Copy, cut, paste (v1) (p. 723)
- Ctrl+C copies.
- Ctrl+X cuts and ripples the gap closed.
- Ctrl+V pastes at the playhead on the original track, overwriting.

### 8.4 Delete (v1) (pp. 721, 724–725)
- **Delete:** removes the selected clips. On Track 1 with Ripple On it ripples, and attached clips follow.
- **With a timeline In/Out set:** Delete removes that range.
- **Disable:** D disables or enables a clip's video. Right-click > Mute silences its audio.

### 8.5 Split (v1) (pp. 646, 723, 950)
- Ctrl+\, the Split button, or Split Clips splits at the playhead.
- Join Match Edit Clips rejoins the halves of a split clip (later).

### 8.6 Trimming (v1 unless noted) (pp. 693, 716, 718, 726–732)
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
- **Trim Start/End to Playhead** (Edit Actions, p. 729): moves a clip's head or tail to the playhead.
- **Change Clip Duration** (Ctrl+D, p. 693):
  - enter frames or a timecode, or pick 1/5/15 seconds
  - End extends to the end of the media
  - "Extend Beyond Clip Length" pads with black (later)
  - applies to every selected clip
- **Dynamic Trim (M7)** (W, pp. 730–732): JKL trim the selected edits while playing.
- **Slide (later):** only exists inside Dynamic Trim.

### 8.7 J/L cuts (v1) (p. 728)
Hover the lower audio part of a clip at an edit until the cursor shows a note, then drag. Only the audio edit point moves, as an audio roll.

### 8.8 Snapping (v1) (pp. 716–717)
- N toggles snapping. Clip edges, markers, the playhead and the timeline In/Out snap to each other, and a white line shows the alignment.
- Pressing N mid-drag is temporary; pressing it between drags sticks.

### 8.9 Markers (v1) (pp. 718–719)
- M adds a marker at the playhead, or edits the marker under it.
- Shift+Up/Down jumps between markers.
- Markers can be dragged.
- Alt+M deletes the marker at the playhead; Delete removes selected markers.

### 8.10 Undo (v1) (pp. 651–652)
- Ctrl+Z / Ctrl+Shift+Z, unlimited.
- A History list shows recent steps and can jump back to one.
- A new change discards the redo steps.

## 9. Audio (pp. 642, 649–650, 743, 771, 773)

- **Meter (v1):** green is safe, yellow is near the limit, red risks clipping.
- **Mixer (M6):** per-track mute/solo, fader and meters. Solo silences the non-soloed tracks, and mute/solo affect export.
- **Clip volume (v1):** one gain value per clip, and clip mute.
- **Ripple Delete Silence (M7)** (p. 773):
  - settings: threshold (dB), pre-head, post-tail, minimum silence (frames), crossfade
  - previews the parts to remove as red stripes; Remove cuts them and closes the gaps

## 10. Quick Export (v1, M5) (Ch. 39, pp. 788–790, 4095–4096, 4104–4109)

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

## 11. Keyboard shortcuts (Linux)

Keys from the Cut chapters, with Edit-page keys filling the gaps (marked †).

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
| F12 | Place On Top (M6) | 890† |
| Shift+F12 | Append | 890† |
| Ctrl+\ | Split at playhead | 723 |
| Delete / Backspace | Delete selection (ripples on Track 1) | 725 |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / cut (ripple) / paste at playhead | 723 |
| Ctrl+A / Ctrl+Shift+A | Select all / deselect all | 719 |
| Alt+Shift+V | Select clips under the playhead | 719 |
| V / U | Select nearest edit / cycle edit side | 720 |
| D | Disable / enable clip video | 724 |
| Ctrl+D | Change clip duration | 693 |
| N | Snapping on/off | 716 |
| M / Alt+M | Add-or-edit marker / delete marker at playhead | 718 |
| W | Dynamic Trim (M7) | 731 |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo | 651 |
| Ctrl+I | Import media | 654 |
| Alt+F / Shift+F / Esc | Enhanced viewer / full screen / exit (later) | 688, 40† |

## 12. Decisions on gaps and contradictions

Each decision applies until the manual or testing shows Resolve behaves otherwise. If testing shows that, update the decision and the code.

- **D1. Fixed playhead.** The lower timeline's playhead is fixed by default, with a Timeline Options toggle. The manual calls it fixed on p. 689 but describes a toggle on pp. 645 and 716.
- **D2. What Split cuts.** If some selected clips are under the playhead, only those are split. Otherwise every clip under the playhead on every unlocked track is split. This is a simplification of the Edit-page Auto Select rule (p. 950). Afterwards, the left halves are selected.
- **D3. Target track.** Smart Insert, Ripple Overwrite and Overwrite use the selected target track (V1 by default). Append always uses Track 1. The manual doesn't say which track Append uses.
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
- **D13. Keys.** Shortcuts the Cut chapters don't give come from the Edit page (marked † in §11). Mac → Linux mapping: Command → Ctrl, Option → Alt.
- **D14. J/L cuts belong to an edit point.**
  - A J/L cut (§8.7) is stored as matching audio offsets on the two clips that meet there: the outgoing clip's `tail` equals the incoming clip's `head`.
  - When an edit gives a clip a different neighbor at that point, the J/L cut resets to a straight cut. Examples: a delete, a move, or a split inside the overlap zone, where one half would otherwise be left without audio of its own.
  - The manual doesn't cover this. Without the rule, audio could overlap on a track or extend into gaps.
