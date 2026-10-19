# Cut page behavioral spec

vidcut copies the behavior of DaVinci Resolve's **Cut page** by default. This document describes that behavior in our own words, with page numbers of the *DaVinci Resolve 21.1 Reference Manual*: its Cut chapters (33–39, pp. 635–790) and, where marked, others. Printed page numbers equal PDF page numbers. Deviations are in [deviations.md](deviations.md); §12 settles points the manual leaves open.

**Reading the manual:** extract text with `pdftotext -f 635 -l 790 <manual>.pdf -`. The developer's copy is in `$VIDCUT_TEST_DATA` (default `~/Downloads/vidcut-tmp`). Don't copy its text into the repo.

**Legend:** **(out)** is out of scope. Cut-page behavior vidcut lacks is listed in [todo/cut-page-gaps.md](todo/cut-page-gaps.md).

## 1. Scope

**In scope:**
- media pool, viewer, dual timeline
- editing and trimming, constant speed changes (D48)
- dissolves: cross dissolves, fades to and from black, audio cross fades (§8.11)
- clip/track audio levels and waveforms
- subtitle tracks: cues, subtitle files in and out, Create Subtitles from Audio (§8.12)
- Quick Export, plus our Export dialog

**Out of scope (out):**
- effects, other transitions, titles, generators, keyframes
- color, Fusion, Smart Reframe, Close Up
- Sync Bin, Multi Source, Source Overwrite, Live Overwrite
- Replay, voiceover
- AI tools other than Create Subtitles from Audio: voice/dialogue tools
- Mixer FX, Paste Attributes, guides
- interchange formats: DRT, AAF, XML

## 2. Layout (pp. 636–637)

- **Main areas:** Media Pool on the left, a single Viewer on the right, the timeline area across the bottom, with a movable seam and timeline height.
- **Top bar:** Media Pool and Mixer tabs (p. 649) on the left, Quick Export and Full Screen viewer on the right. (Sync Bin, Keyframe Editor, Transitions, Titles and Effects tabs and the Inspector: out.) vidcut's bar also has Import, Export and Jobs (deviation X3), Help (X23) and the command log (X5).
- **Below the Media Pool:** the edit-function buttons (§7), Cut and Dissolve (§8.11).
- **Viewer:** mode buttons top-left, Duration field top-right; along the bottom (p. 640) the jog control, Previous Edit, Stop, Play, Next Edit, Loop, Mark In, Mark Out and timecode; an audio meter on the right (p. 642).
- **Timeline menus** (pp. 645–646):
  - Timeline Options: Ripple On, Snap, Fixed Playhead, Trim to Audio, Trim with Safe Edit, Display Clip Names, Durations and Status, Minimize Subtitle Track, Boring Detector.
  - Timeline Actions: Add Video/Audio/Subtitle Track, Split Clips, Join Match Edit Clips, Add Marker, Detect Scene Cuts, Add Dissolve to All Edits, Clear Transition from All Edits (vidcut: Add Subtitle too, deviation X44, and Create Subtitles from Audio, X46).
  - Edit Actions: Trim Start to Playhead, Trim End to Playhead.
- **Timeline buttons** (p. 649): Ripple On, Dynamic Trim, Split, Marker.
- **Upper timeline:** track-number buttons to its left select the target track; the Audio Only and Video Only toggles sit there too (pp. 643, 694).

## 3. Media Pool (Ch. 34, pp. 653–675)

- **Import** (p. 654):
  - Ctrl+I, the Import Media button, or right-click > Import Media opens a file dialog with multi-select.
  - Import Media Folder, the button beside Import Media, opens a folder dialog and brings the folder in as a bin inside the open bin, with a bin for each folder in it, and opens that bin (pp. 654, 658; D53). Resolve Studio 21.1.1's Cut page has no File-menu or right-click item for it, and Keyboard Customization lists no command by that name; vidcut adds both items (deviation X25).
  - In a project created without a format, the first video sets it without asking (D37).
- **Stills and image sequences** (pp. 114, 366, 1194): an image file, or a GIF or WebP of one frame, comes in as a still (D38); numbered image files, as one clip (D39).
- **Remove** (p. 654): Delete or Backspace, after a confirmation. Only the pool entry goes; files on disk stay.
- **Views** (pp. 638, 664–665):
  - **Thumbnail:** hovering scrubs the clip, and I/O mark its source In/Out. Clicking a thumbnail's lower-right corner shows its metadata.
  - **Filmstrip:** each clip is a row of its pictures as wide as the pool, under its name; hovering and I/O work as on a thumbnail.
  - **List:** a column per field; clicking a header sorts by it.
  - **Metadata:** a card per clip, with a scrubbable thumbnail and basic metadata.
  - A sound file shows its waveform.
- **Sort and search** (pp. 666–667): Sort Media By Clip Name, Date Imported, Date Modified or Timecode (D59), ascending or descending, which is also the Source Tape order. The search field filters live, through the open bin and the bins inside it.
- **Bins** (pp. 655–656, 377): the Master bin is the top level. The pool shows the open bin's clips and its bins as folders; a path at the top (Master › Interviews › Day 1), the Bins menu and a double-click open any bin. New Bin is on the File menu (Shift+Ctrl+N), the Bins menu and the pool's right-click menu; a bin's own menu has Rename Bin and Remove Bin, which keeps its clips (deviation X6). Clips dragged onto a bin, or a level of the path, move into it, as do bins; a clip's menu has Move to Bin and Create Bin With Selected Clips (p. 656). Imports go into the open bin (p. 377). Source Tape strings together the open bin's clips and its bins' clips, in the sort order (pp. 680, 696).
- **Clip Color** (pp. 667–668): right-click clips in the pool or the timeline and pick one of 16 colors, or Clear Color. A timeline clip is tinted with its own color, else its pool item's.
- **Relinking media** (pp. 668–669): an item whose file is missing is marked MISSING, its clips are red on the timeline (pp. 509, 530), and the viewer shows Media Offline. Relink Media finds the files in a chosen folder and its subfolders; a clip's Relink… picks one. vidcut checks each file it finds (deviation X1).
- **Interaction:** a click selects; in Source Clip mode the Viewer shows the clip, in Source Tape mode the playhead jumps to it (pp. 680, 704). A double-click opens it as Source Clip (p. 680).
- **Selection** (p. 719): click, Shift-click range, Ctrl-click toggle, marquee, Ctrl+A.

## 4. Viewer (pp. 639–640, 679–693, 713)

### 4.1 Modes
- **Source Clip** (p. 680): the selected pool clip. A scroll area under the picture stands for the whole clip, with the playhead, a zoomed-in waveform and draggable In/Out handles. Jog In and Jog Out controls at its ends move the marks frame by frame (p. 692).
- **Source Tape** (pp. 680–681, 704–706): all pool clips play end to end as one tape, in pool sort order. The clip under the playhead becomes selected in the pool.
- **Timeline** (p. 681): the timeline at its playhead, with no scroll area; icons mark a clip's first and last frame.
- **Switching:** Q toggles between Timeline and the last source mode (p. 696); any click or drag in the timeline area switches to Timeline (p. 681).

### 4.2 Transport (pp. 640, 687–688, 870–872)
The keys are in §11. J/K/L shuttle: repeated J/L speeds up, 2× to 64×, K held with J or L plays slowly, and K held with a tap of J or L steps a frame (pp. 696, 871; D54). Ctrl+/ loops playback (D51), and dragging the jog control scrubs finely (p. 640).

### 4.3 Timecode (pp. 640, 688, 713–714)
- **Go to a time:** click the timecode field and type, or type `=` and a timecode, or `+` or `-` and a value to jump by. Digits fill a timecode from the right: `100` is 1 second, `10000` 1 minute. A value with a unit (`5s`, `450f`) is that duration.
- **Display:** source timecode in the source modes, record timecode in Timeline mode, drop-frame where the project counts so (D47). Right-clicking offers the source or record frame count, Timer 1 and 2, and KeyKode.

### 4.4 In/Out (pp. 691–693, 700)
- **Source In/Out:** I/O in a source mode or over a pool thumbnail, or the scroll area's handles.
- **Timeline In/Out:** I/O in Timeline mode, or while dragging either timeline's ruler. The range shows on both rulers and can be dragged there.
- **Clearing:** Alt+X clears In and Out.
- **Duration field:** the marked duration. Editing it moves the Out mark: a number of frames, + or − a number to change it by, or a timecode (p. 693; D60).

### 4.5 Other viewer features
- Full Screen (Shift+F) and Enhanced viewer (Alt+F); safe areas; Fast Review, which plays long clips faster than short ones (p. 640); skimming with C held (p. 640).
- Audio scrubbing (Shift+S; on by default, p. 876): dragging the playhead, and frame steps, play the sound under it (D42). Timeline > Audio Scrubbing switches it too.

## 5. Dual timeline (pp. 66, 643–646, 689–692, 716–718)

**Upper timeline:** the whole program at full width, with no zoom. Its playhead moves freely: dragging its ruler, or a sideways two-finger swipe, scrubs the program. Most edits work there, clips can be dragged between the two timelines, and it shows the timeline In/Out and markers.

**Lower timeline:**
- A detail view around the playhead (p. 644). Ctrl+wheel (deviation X21), a trackpad pinch and the − and + buttons zoom it: the manual calls its zoom fixed, but Resolve zooms it too. A sideways swipe scrolls it (p. 66).
- **Fixed Playhead on (default, D1):** the playhead stays at the center and the clips move under it, also when the ruler is dragged or swiped. Clips don't snap to it (p. 717).
- **Fixed Playhead off:** the clips stay put and the playhead moves; the view pages when it reaches an edge. Clicking the ruler moves the playhead, and it can be dragged.

**Track display:** combined video+audio tracks are numbered 1, 2, …, with audio-only tracks A1, A2, … below them. A video clip shows a filmstrip, each picture the frame under it, the first its first frame, and its audio as a waveform band along the bottom, enlarged with the track (p. 647); its duration shows as seconds:frames, bottom-left. A locked track's clips are stippled, a disabled track's dimmed; a disabled clip is dull; a muted clip shows a mute icon.

**Track headers** (pp. 647, 678, 717): Enlarge (one track at a time), Lock, Mute (audio) and Video Enable icons (D43).

**Automatic tools** (p. 690): the **Boring Detector** (Timeline Options) highlights clips longer than "Boring Clips" (light gray) and shorter than "Jump Cuts" (red) on the upper timeline, live as you edit; Analyze Timeline Edits sets both (D31). **Detect Scene Cuts** (Timeline Actions) splits the selected clips where their shots change (D30).

## 6. Track model (pp. 647, 649, 716, 721, 727)

- **Track 1 is the main story (A-roll):** insert, delete, move and trim on it ripple, so it never has gaps. With **Ripple On** off it behaves like the other tracks, and Alt held while trimming turns rippling off for that trim.
- **Tracks 2+ are B-roll:**
  - each clip is attached to the Track 1 clip under its In point and follows it when Track 1 ripples (p. 649; D21)
  - trimming the anchor's Out to before the attached clip's In deletes the attached clip (p. 727)
  - they ripple only where there is no Track 1 video under them (p. 721; D22), and clips sit freely with gaps
- **Audio tracks:** A1, A2, … below the video tracks hold sound alone, such as music. Media without video go there by default (D26).
- **Track header** (p. 647): a click makes the track the target (D3); right-click adds a video or audio track or removes one (D27).
- **Overlapping video:** the highest enabled track is visible (p. 647). A disabled clip, or a track with its video off, shows what is under it.
- **Linking:** a clip's video and audio are one combined clip. Trims move both, except that a J/L cut moves only the audio edit (§8.7). There is no link/unlink on the Cut page.

## 7. Edit functions (pp. 696–701)

**Workflow** (p. 696): pick a source, mark its In/Out, optionally click a track's header or number to target it, press an edit button or key, then Q to review the result and Q again to go back. Video Only and Audio Only limit what an edit takes (p. 694).

### 7.1 Smart Indicator (p. 696)
A marker on the ruler shows the edit point nearest the playhead, which some functions use instead of the exact playhead frame. After idling it bobs, and the chosen edit flashes green, or red if the clip there has no media beyond its edge. vidcut shows the colors without the motion (deviation X15).

### 7.2 Edit functions
- **Smart Insert** (F9 in the Edit page; p. 697): inserts at the Smart Indicator's edit point on the target track, never at an arbitrary frame. On Track 1, everything after it moves right by the incoming duration.
- **Append** (Shift+F12; p. 697): ignores the playhead and places the clip after the last clip.
- **Ripple Overwrite** (Shift+F10; p. 698): with no timeline marks, replaces the whole clip under the playhead; with a timeline In/Out, exactly that range, possibly across clips. Everything after shifts by (incoming − replaced) duration, either way: the source duration always wins.
- **Overwrite** (F10; p. 701): writes over the target track at the playhead, at the timeline In, or backtimed to a lone timeline Out. Other clips don't move.
- **Place On Top** (F12; pp. 699–700): goes to the first track above every clip at that position, creating one if needed (D24), whatever the target track, and never ripples. It lines up, in this order of priority:
  1. a timeline In with the source In;
  2. a lone timeline Out with the source Out (backtimed);
  3. with no marks, within 5 frames of an edit, with that edit;
  4. otherwise with the playhead.
- **Close Up, Source Overwrite (out).**

### 7.3 Drag and drop from the pool or Viewer (pp. 648, 679, 694–695)
- **Onto an empty timeline, or left of the first clip:** becomes the first clip.
- **Past either end of all clips:** appended or prepended.
- **Onto a clip, released right away** (the whole clip highlights): Ripple Overwrite replaces that clip.
- **Onto a clip after hovering briefly** (the highlight narrows to the incoming footprint): a plain Overwrite with no ripple, which may split the target.
- **Onto an edit point:** insert at that edit.
- **Into the gray area above the top track:** creates a new video+audio track. Below the bottom track, an audio clip creates an audio-only track.
- **Onto another track:** an overwrite there, never a ripple (only Track 1 pushes; D23).

## 8. Timeline editing (Ch. 36, pp. 715–732)

### 8.1 Selection (pp. 719–720)
- **Clips:** click selects, Ctrl+click toggles, Shift+click selects a range, a marquee from empty space selects what it touches. Clicking empty space, or Ctrl+Shift+A, deselects. Alt+Shift+V selects every clip under the playhead.
- **Edit points:** the pointer at the center of an edit selects both sides (a roll), just left or right of it one side (a resize); Ctrl+click adds edits. V selects the edit nearest the playhead; U cycles both, incoming, outgoing. In both timelines the selected edit is a solid line on the cut with a glow on its selected side, or both, pulsing about once a second: green where that side has media to trim into, red where it has none.

### 8.2 Moving clips (pp. 720–723)
- **Onto another Track 1 clip, released quickly:** replaces it and ripples.
- **After hovering first:** overwrites the middle of the target, splitting it; the gap left behind closes on Track 1.
- **Over a neighbor's edge, the pointer not reaching it** (p. 722): the overlapped part of the neighbor is overwritten and the gap left behind closes.
- **Onto an edit point** (it highlights): insert-at-edit. The gap left behind closes, and later clips shift with their attached clips.
- **Track 1 clip dropped onto another track:** can only overwrite (p. 721); Track 1 closes up behind it.
- **Higher-track clips:** move freely and leave gaps. Moves between tracks: D25.

### 8.3 Copy, cut, paste (p. 723)
Ctrl+C copies. Ctrl+X cuts and ripples the gap closed. Ctrl+V pastes at the playhead on the original track, overwriting.

### 8.4 Delete (pp. 721, 724–725)
Delete removes the selected clips; on Track 1 with Ripple On it ripples, and attached clips follow. With a timeline In/Out set, it removes that range. D disables or enables a clip's video; right-click > Mute silences its audio.

### 8.5 Split (pp. 646, 723, 950)
Ctrl+\, the Split button, or Split Clips splits at the playhead. Join Match Edit Clips (Timeline Actions) joins a split clip's parts back into one (D49).

### 8.6 Trimming (pp. 693, 716, 718, 726–732)
- **The pointer position picks the operation:**
  - **Clip edge: resize.** A tooltip shows the frame delta and new duration. On Track 1 with Ripple On it ripples; elsewhere only that clip changes.
  - **Edit center: roll.** The total duration stays; a white overlay shows the available handle media.
  - **Slip handle at the clip center: slip.** Several selected clips slip together.
  - **Lower audio band at an edit: audio-only roll**, a J/L cut (§8.7).
- **Safe Edit** (option, p. 732): a trim stops at the neighboring clip, with a small timer on the cursor; keep dragging to overwrite anyway.
- **Trim to Audio** (option, p. 645): the audio view expands while trimming.
- **Trim Editor** (pp. 729–730; D52): while an edit is selected and the player is stopped, the Viewer shows the outgoing clip's last frame beside the incoming clip's first, over the outgoing filmstrip on top and the incoming one below. Each frame is numbered from where the edit was when the editor opened, with a bar in the clip's color under the clip's own frames and a grey one under those past its edge. The handles stay at the centre while the filmstrips slide under them, as far as the media allow: the top handle trims the outgoing Out, the bottom one the incoming In, the white one between them rolls. A handle turns red where its side has no more media. A pair of ±1-frame buttons each side of the transport nudges the Out (left) and the In (right), or rolls with both sides selected. An edit with a dissolve shows the dissolve between the filmstrips, several frames to a cell; dragging either end makes it longer or shorter about the edit. Esc or Close deselects the edit.
- **Trim Start/End to Playhead** (Edit Actions, p. 729): moves a clip's head or tail to the playhead.
- **Change Clip Speed** (R†): a speed, reversed or not, or a freeze frame, for every selected clip (D48).
- **Change Clip Duration** (Ctrl+D, p. 693): frames or a timecode, or 1, 5 or 15 seconds, for every selected clip. End runs to the end of the media; Extend Beyond Clip Length pads with black.
- **Dynamic Trim** (W, pp. 730–732): W toggles the mode and shows the timeline. The playhead turns yellow and goes to the selected edit (Out, In or both by U; else the one nearest the playhead), which the Trim Editor shows, or to the selected clip's edge nearer the playhead. J/K/L then trim the edit, or slip or slide the selected clips (right-click the button to choose), instead of playing; with nothing selected they play. A slide keeps a clip's media and lengthens the clip before it as it shortens the one after (p. 731); L slides it right, and slips its content right, to earlier frames, as dragging it does. Space plays around the playhead and Up/Down go to another edit (D32, deviations X11, X28, X29). W again deselects the edit.

### 8.7 J/L cuts (p. 728)
Hover the lower audio part of a clip at an edit until the cursor shows a note, then drag: only the audio edit moves, as an audio roll. A click selects that audio edit, and `,` / `.` move it (D20).

### 8.8 Snapping (pp. 716–717)
N toggles snapping. Clip edges, markers, the playhead and the timeline In/Out snap to each other, shown by a white line. N pressed mid-drag lasts for that drag; between drags, it sticks.

### 8.9 Markers (pp. 718–719)
M adds a marker at the playhead, or edits the one there. Shift+Up/Down jumps between markers, which can be dragged. Alt+M deletes the marker at the playhead; Delete removes selected markers.

### 8.10 Undo (pp. 651–652)
Ctrl+Z / Ctrl+Shift+Z, unlimited. Edit > History lists the last twenty steps, newest on top, the current one checked and the steps that can be redone still listed; choosing one undoes or redoes to it. Open History Window, at the foot of that menu, lists them all, oldest on top from Original, the project as it was before them: the current step has a dot, the steps that can be redone are greyed, and a click on any step goes there at once. A new change discards the steps that could be redone. Details: D55.

### 8.11 Dissolves (pp. 645, 747, 758–761, 895, 1114, 1159, 1173)
- **Dissolve and Ctrl+T** (p. 758): a one-second Cross Dissolve centered on the edit nearest the playhead. Ctrl+T puts it on the selected edit (V) instead, else on the selected clips (p. 895), else on the edit nearest the playhead. U picks where: ending on the edit (outgoing side), starting on it (incoming) or centered (both). Where the clips' sounds meet at that edit, a Cross Fade goes on the sound too (p. 1114). Details: D50.
- **Cut** (p. 759): the edit nearest the playhead becomes a cut again.
- **A selected dissolve** (pp. 759, 761): a click selects it. Its right-click menu has Change Transition Duration, then Start on Edit, Center on Edit and End on Edit, the current one greyed. Change Transition Duration takes a timecode or frames (Format, remembered) or a 1, 5 or 15-second preset, and is shortened to the media (deviation X27); Ctrl+D opens it too. Delete removes the dissolve. Dragging a dissolve's edge, in either timeline, makes it longer or shorter evenly about the edit at that timeline's scale, shortened to the media (X27), with a tooltip of the edge's move and the new length, and selects it. Each acts on the Cross Fade at the edit as well.
- **How it plays** (pp. 747, 760, 1159, 1173): each clip shows its media past its edge, so a dissolve is as long as the clips have frames there. Pictures cross in a straight line, at a gap from or to black; a Cross Fade's level at its middle is +3, 0 or −3 dB.
- **Out:** other transitions (wipes, Smooth Cut, the Transitions browser), the Transition Inspector, moving a transition to another edit, Change Transition Type, Set as Default Transition, and cutting, copying and pasting transitions.

### 8.12 Subtitles (pp. 645–646, 702, 1227–1248)
- **Subtitle tracks** (pp. 646, 702, 1228, 1232, 1245): Add Subtitle Track (Timeline Actions, or a track header's right-click menu) adds one above the video tracks, named Subtitle 1, 2, …; double-clicking its name renames it, and exports use the name. A subtitle track locks like any track. One subtitle track shows at a time: showing one hides the others (p. 1228). Minimize Subtitle Track (Timeline Options, p. 645) makes them shorter. Details: D56; deviation X44.
- **Cues** (pp. 1232–1233, 1237): Add Subtitle (a subtitle track's right-click menu) puts a cue at the playhead, and its text is typed in the Inspector, showing on the cue as it is typed. Cues move, trim, split and delete like clips. Details: D56, D57; deviations X43, X44.
- **Import** (pp. 1230–1231): `.srt`, `.srtx`, `.vtt`, `.xml` and `.ttml` files come into the Media Pool as subtitle clips. Insert Selected Subtitles to Timeline Using Timecode puts each cue at its timecode; dragging one onto the timeline puts its first cue where it is dropped. Details: D58; deviations X40, X41.
- **Export** (pp. 1247–1248): File > Export Subtitle, or a subtitle track header's Export Subtitle, writes a track as SRT or WebVTT. On the Deliver page a render writes each chosen track as a file beside it (IMSC1, DFXP, SRT, WebVTT), burns the shown track in, or embeds it. Details: D58, D61; deviations X42, X45.
- **Create Subtitles from Audio** (pp. 1234–1235): transcribes the speech of the timeline's In–Out range, or of all of it, onto the highest subtitle track, made when there is none. Its options: the language (Auto finds it), a caption preset, the characters per line (42), one or two lines, and the frames between subtitles (0). A muted track isn't heard. Details: D62; deviation X46.
- **Out:** styles (the track's, a cue's, a word's) and style presets, caption presets, fades, subtitle regions, linking subtitles to clips, Fusion and animated subtitles, word timing, find and replace, QC thresholds, the Captions list, hiding subtitle tracks from the timeline (pp. 1228–1229, 1231–1232, 1237–1244). Track styles: [todo/subtitles.md](todo/subtitles.md).

## 9. Audio (pp. 642, 649–650, 743, 771, 773)

- **Meter:** green is safe, yellow near the limit, red risks clipping.
- **Mixer** (pp. 649–650): a channel strip per track with sound (name, meter, fader in dB, mute and solo) and the mix's meter. Solo silences the tracks that aren't soloed; mute and solo apply to the export too (p. 650). Track pan and FX: out.
- **Viewer meter** (p. 642): shows what the viewer plays; its speaker button mutes playback only, not the export.
- **Clip volume:** one gain per clip, and clip mute.
- **Ripple Delete Silence** (p. 773): Clip > Audio Operations > Ripple Delete Silence on the selected clips (vidcut: the clip's right-click menu or Timeline Actions). Settings: threshold (dB), pre-head, post-tail, minimum to trip; vidcut adds Detect, Sound level or Speech (deviation X47). It shows the parts to remove as red stripes; vidcut lets them be kept, trimmed and played first (deviation X48). Remove cuts them and closes the gaps, as one undo step (D29; no crossfade option, deviation X11).

## 10. Quick Export (Ch. 39, pp. 788–790, 4095–4096, 4104–4109)

- **Flow:** File > Quick Export or the toolbar button, then a preset from the icon row, Export, a file name and a progress dialog.
- **Range:** the timeline In–Out range if set, else the whole timeline.
- **Output:** one file; individual clips are the Deliver page's, and our Export dialog's.
- **Preset models** (Deliver page): H.264 Master (QuickTime, AAC 320 kb/s), H.265 Master (Main10), ProRes 422 HQ (LPCM audio), YouTube 720p–2160p (MP4, H.264 High), Vimeo, TikTok (vertical 1080×1920).
- **User presets** can be added to the Quick Export row.
- **In vidcut:** the Quick Export row, the Export dialog and jobs are deviation X3. The suggested file name is the project's name.

## 11. Keyboard shortcuts

Keys from the Cut chapters (pages in §4–§10), with Edit-page keys filling the gaps (†; D13). The manual's Mac keys are named as on Linux and Windows, Command as Ctrl and Option as Alt; macOS keeps the manual's.

Shortcuts do nothing while a text field has focus or a modal question is up; Esc dismisses the question without acting (Cancel, or Keep Running when quitting).

| Key | Action |
|---|---|
| Space | Play / stop |
| J / K / L | Shuttle back / stop / forward |
| K held + J/L held, or a tap of J/L† | Half-speed play / single-frame step (D54) |
| ← / → (Shift: 1 s)† | Step a frame |
| Up / Down | Previous / next edit |
| Shift+Up / Shift+Down | Previous / next marker |
| Home / End† | Timeline start / end |
| Ctrl+/ | Loop playback |
| I / O | Mark In / Out |
| Shift+I / Shift+O† | Go to In / Out |
| Alt+X | Clear In and Out |
| = / + / − | Go to timecode / relative jump |
| Q | Viewer: Timeline ↔ last source mode |
| F9† | Smart Insert |
| F10 | Overwrite |
| Shift+F10† | Ripple Overwrite |
| F12† | Place On Top |
| Shift+F12† | Append |
| Ctrl+\ or Ctrl+B | Split at playhead: the selected clips and subtitles there, or with nothing selected, every clip there |
| Ctrl+T | Dissolve (§8.11) |
| Delete / Backspace | Delete (ripples on Track 1); a selected dissolve; selected subtitles |
| Alt+Delete | Lift the selection (D9) |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / cut / paste |
| Ctrl+A / Ctrl+Shift+A | Select all / deselect all |
| Alt+Shift+V | Select clips under the playhead |
| V / U | Select nearest edit / cycle edit side |
| , / . (Shift: 1 s)† | Trim the selected edit a frame |
| D | Disable / enable clip video |
| Ctrl+D | Change clip or dissolve duration |
| R† | Change clip speed (D48) |
| Enter | Edit the selected subtitle (X43) |
| N | Snapping on/off |
| M / Alt+M | Add or edit / delete marker |
| Click an edit | Trim Editor (D52) |
| W | Dynamic Trim on/off |
| Shift+S | Audio scrubbing on/off |
| Ctrl+wheel | Zoom the lower timeline (X21) |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo |
| Ctrl+I | Import media |
| Shift+Ctrl+N | New bin |
| Alt+F / Shift+F / Esc† | Enhanced viewer / full screen / exit |

## 12. Decisions on gaps and contradictions

Each decision holds until the manual or testing shows Resolve behaves otherwise; then the decision and the code change.

- **D1. Fixed playhead.** The lower timeline's playhead is fixed by default, with a Timeline Options toggle: the manual calls it fixed (p. 689) but describes a toggle (pp. 645, 716).
- **D2. What Split cuts.** The selected clips under the playhead, if any, else every clip under it on every unlocked track: a simplification of the Edit page's Auto Select (p. 950). The left halves end up selected.
- **D3. Target track.** Smart Insert, Ripple Overwrite and Overwrite use the target track (V1 by default; A1 for sound alone, D26). Append, which the manual leaves open, always uses Track 1; Place On Top picks its own (D24).
- **D4. Trim Start/End to Playhead.** Applies to the selected clips; with none, to the Track 1 clip under the playhead.
- **D5. Missing source marks.** No source In means the clip's first frame, no Out its last. In Source Tape mode with no marks, an edit takes the whole clip under the playhead.
- **D6. Marks persist after edits.** Source marks are kept per clip and timeline marks until cleared (Alt+X): the least surprising for scripts.
- **D7. Overwrite with timeline In and Out set.** A three-point edit: the range sets the duration, from the source In. Too little source media is an error.
- **D8. Ripple Overwrite with timeline In and Out set.** The source duration wins, and the program ripples by the difference (p. 698).
- **D9. Lift on Track 1.** Alt+Delete lifts, leaving the gap, as Alt suspends ripple while trimming; with Ripple On off, Delete does too.
- **D10. Overwrite past the end of Track 1.** With Ripple On it starts at the end instead, so Track 1 stays gapless.
- **D11. Hover time.** 0.8 s of hovering turns a drop or move onto a clip from replace into overwrite. The manual gives no value.
- **D12. Clip gain range.** −60 dB to +24 dB, plus mute. The manual gives none.
- **D13. Keys.** Keys the Cut chapters don't give come from the Edit page (marked † in §11).
- **D14. J/L cuts belong to an edit point.** A J/L cut (§8.7) is the outgoing clip's `tail` matching the incoming clip's `head`. An edit that gives a clip another neighbor there resets it to a straight cut, as it does the clip's other cut if the clip would be left without sound of its own. Otherwise audio could overlap on a track or run into gaps.
- **D15. Playhead after an edit function.** After Smart Insert, Append, Overwrite, Ripple Overwrite or Paste, the playhead moves to the frame after the new clips, and the Viewer keeps its mode, so Q reviews the result (p. 696). A drop leaves it alone. This is the Edit page's behavior.
- **D16. Mark Out includes the frame under the playhead,** as in Resolve. Ranges are stored half-open: O at frame `f` stores `out = f + 1`.
- **D17. Source Tape In/Out** (p. 692). I/O in Source Tape mode mark the tape, kept as tape frames with that moment's pool order, so an edit takes the same span whatever the pool shows later; within one clip they are also its source marks (D5). An edit places the span as one block, in one undo step. Removing or relinking one of its clips clears them.
- **D18. Drop and move targets.** Within 6 px of an edit point a drop inserts there; several clips always overwrite. Up to 24 px below the bottom row makes an audio track. Further off the tracks, or Esc, cancels.
- **D19. Trimming with the pointer.** Edges follow the pointer's movement from the press, snapped (§8.8). A trim, roll, slip or J/L cut stops where the media runs out instead of failing. Safe Edit (p. 732) holds an edge at the neighbor for 0.6 s; pushing on overrides it. Dragging empty space scrubs in the upper timeline and draws a marquee in the lower.
- **D20. Keyboard trimming.** With an edit selected (V, U or a click), `,` and `.` trim it a frame, with Shift a second: the Out, the In or both. They are the Edit page's nudge keys; the Cut page's own keyboard trimming is Dynamic Trim (D32). On a J/L handle they move the audio edit and leave the picture cut (D36).
- **D21. Following Track 1** (pp. 649, 727). A clip on another track is attached to the Track 1 picture under its first frame. It moves with that picture, split parts included, and goes where Track 1 closed up when that clip is deleted. Trimming that clip's tail past it deletes it (p. 727), its head moves it to the new start, and replacing the clip leaves it in place. Over a gap it keeps its distance to the Track 1 clips around it. Clips on locked tracks don't follow; one that would land on another clip moves to the next free track of its kind.
- **D22. Rippling other tracks** (p. 721). With Ripple On, deleting or trimming a clip on another track ripples that track only when Track 1 has no clips from that clip's start on; otherwise it leaves a gap. Deleting a timeline In–Out range takes it out of every unlocked track and closes them together (p. 725).
- **D23. Edit functions on other tracks.** Only Track 1 pushes clips (p. 697). Elsewhere Smart Insert and insert overwrite, and Ripple Overwrite replaces without moving anything else. Paste puts each clip back on its own track (p. 723), creating missing ones. Split follows D2.
- **D24. Where Place On Top goes** (pp. 699–700). Onto the video track above the highest one with clips in the incoming span, created if needed; onto Track 1 only if nothing is there (D10 applies). Sound alone goes to the first audio track with room.
- **D25. Moving clips between tracks.** Moves are given in positions from before the move. A Track 1 clip moved to another track lifts out, and with Ripple On Track 1 closes up. Clips moved onto Track 1 insert or overwrite (§8.2). Video can't go onto an audio track, and a selection across tracks moves only along them.
- **D26. Sound alone.** An edit without picture (media without video, or Audio Only, p. 694) goes to A1 unless a target track is given; dragged onto a video track, it lands on the first audio track. A clip on an audio track keeps only its audio.
- **D27. Track numbers are positions.** V1, V2, … count from the bottom, A1, A2, … from the top; removing a track renumbers those after it, and removing one with clips must be confirmed. An edit or move may name the next track to create it. Track 1 can't be removed. Track volume runs from −60 to +24 dB (D12).
- **D28. Meters.** A bar per channel of the project's channel count (p. 649): linear peaks from −60 dBFS, held 1.5 s. Playing, they show the sound as heard, the mix and each track after its fader; paused or scrubbing, the media's waveforms at the playhead, with the gains. Channels mix, and mute and solo apply, as in the export (p. 650). The monitoring mute silences only the viewer meter.
- **D29. Ripple Delete Silence** (p. 773). The manual gives no defaults or rule. Silence is where every channel's peak after clip gain stays below the threshold, in 10 ms steps, including a clip past its audio's end and a click (under 0.05 s between 0.1 s of silence). Defaults: −40 dBFS, pre-head and post-tail 0.15 s, minimum 0.5 s, in seconds so the frame rate doesn't change them. Each silence of at least the minimum goes, less post-tail and pre-head (all of it at a clip's ends), in whole frames. It ripples whatever Ripple On says. Remove takes the stripes as they are left (deviation X48).
- **D30. Finding shots and black.** Detect Scene Cuts (p. 690) gives no algorithm. A frame starts a shot when FFmpeg scdet's score on its luma reaches 10 (0–100: the mean difference from the frame before, or its change if smaller, so steady motion isn't a cut). A frame is black when 98% of its pixels are within 10% of black; black of 0.5 s or more is reported.
- **D31. Boring Detector defaults** (p. 690). The manual gives none. Clips over 30 s are boring and clips under 10 frames are jump cuts. It looks at video tracks; its settings are view options kept between runs, not part of the project.
- **D32. Dynamic Trim** (pp. 730–732). L or J runs the trim at shuttle speed (repeat for 2× up to 64×) until the media or a neighbor runs out, playing with sound at that speed what the trim changes under the playhead: a roll or the In side plays the incoming clip on from the cut, the Out side the outgoing clip, a slide the clip beside the edge the playhead is on, and a slip the clip alone as the timeline plays it, backward when L moves it right; Loop doesn't wrap it. The playhead goes to the edit, or the first selected clip's edge nearer it, when the mode starts and when a run starts, and moves with the edit, or a sliding clip's edge, as it runs; on the In side (Ripple On) and in a slip it stays. While it runs, the viewer shows the exact frames on both sides of the edit, or for a slip or slide four: the clip's first and last frames over the frames before and after it; the timeline shows the clips as the trim leaves them. Stopped, an edit shows the Trim Editor and clips the viewer. K, Space, W, Up/Down and the end of the media stop a run and apply it as one request, one undo step (deviation X11); where the media or a neighbor ran out, the cut turns red and the viewer says why (deviation X29). K held with a tap of J or L trims one frame, and with J or L held trims at half speed until one is let go (D54); `,`/`.` trim as usual (D20). Stopped, Space plays from 2 s before the playhead to 2 s after and puts the playhead back.
- **D33. Rotation metadata.** The manual doesn't say. As in players, a display-matrix rotation (phones store portrait video sideways with one) is applied everywhere, so a portrait clip that sets the format makes a portrait project. Mirrored matrices count as rotations only.
- **D34. Interlaced sources** (p. 741). A stream is interlaced if its field order says so or its first frame is flagged so. Flagged frames are deinterlaced everywhere with `bwdif`, one frame per frame, the first field kept (deviation X13). Output is never interlaced, nor at the field rate.
- **D35. HDR and color** (pp. 230–239). A clip is HDR when its transfer is PQ or HLG. SDR output tone-maps it to BT.709, keeping up to 80% of the 203-nit reference white and rolling highlights off up to 1000 nits. HDR output (D44) is BT.2100 in 10 bits, with SDR white at 203 nits (BT.2408), as Resolve's "203 nits" option (p. 238). Untagged media are BT.709 when 1280 wide or over 576 lines, else BT.601, as players guess.
- **D36. Which track an edit point is on** (pp. 696, 720, 729–732). The Smart Indicator, V, Smart Insert, Place On Top's alignment and Up/Down use Track 1's edits, the main story. An edit picked by clicking stays on its track for `,`/`.`, U, the Trim Editor and Dynamic Trim.
- **D37. The format: chosen at the start, or set by the first video without asking** (§3; Resolve asks, p. 654). New Project takes any even picture size from 16 px and any positive frame rate. Without them, the first media with video sets both, with no prompt to stop scripts and agents. The size can change at any time, the rate only while the timeline is empty, as positions are whole frames.
- **D38. Stills** (§3; pp. 114, 1194). The manual gives the standard still duration, 5 s (p. 114), and 10 s (p. 1194); vidcut takes 5 s, as a project setting (deviation X16). A still edited without a range lasts that long; a range or a trim makes it up to 17 h 40 min (p. 1194). Every source frame of a still shows its picture, so only that length bounds its clips, a dissolve's frames included: a head trim or a dissolve can reach before its source start. A clip keeps its source position, which can fall below 0. A GIF or WebP of one frame is a still too; one of more frames is a clip. Resolve Studio 21.1.1 imports a WebP of one frame as a still, but a GIF of one frame only when its frame has no delay (deviation X39). With FFmpeg 8, which can't decode an animated WebP, one comes in as a still that shows nothing ([to-do](todo/animated-webp-ffmpeg-8.md)).
- **D39. Image sequences** (§3; p. 366). `media.add` with `sequence` brings numbered files (`frame_000001.png`, …) in as one clip, the unbroken run around each file given, at the project's rate unless `fps` says otherwise; `media.add_folder` with `sequence` does so for each run of two or more in its folders. The pool names it as Resolve does: `frame_[000001-000120].png`. Without `sequence`, a numbered image is a still (deviation X17).
- **D40. Timeline interchange with Resolve** (deviation X9; pp. 527–530, 4160). The manual describes Resolve's OTIO import and export, not the files. vidcut's OTIO takes the shapes Resolve writes and needs (source ranges in media timecode, plain paths, link groups, marker notes), so a cut goes to Resolve and back with its media linked and its audio with its video. Mapping: [deviation X9](deviations.md#x9-opentimelineio-interchange), [model](design/model.md#opentimelineio).
- **D41. Proxies** (deviation X18; the proxy chapters, pp. 207–218, are outside the Cut page). Videos and image sequences get proxies on demand (Generate Proxy), or from a running GUI for media hard to play: image sequences, pictures over 3840×2160 and files over 150 Mb/s (preferences `proxies.*`), checked as media are added, when `use_proxies` turns on, and when a proxy's file is gone. A proxy whose file is gone counts as none: the pool marks it LOST PROXY, the original plays, and these rules make it again as the project names it: its size, and its codec where this computer makes it. A proxy's longest side is 960 px unless set; `auto` picks a codec the GPU plays ([export](design/export.md#proxies)). A project names a proxy by its file, which each computer keeps in its own proxy folder: `proxies` in the project's folder, or where `proxies.folder` says; a relative folder is in the project's folder, an absolute one is shared by every project (deviation X26). With `use_proxies` on, the preview plays fresh proxies, frame for frame as the originals, so heavy media play in real time; sound, exports and analyses use the originals.
- **D42. Audio scrubbing** (§4.5; p. 876). The manual says only that dragging the playhead plays the sound. A frame step, and every move of a paused playhead, plays that frame of the mix, faded over 3 ms against clicks, and forward even when stepping back: a reversed frame isn't heard as backward. A newer frame replaces any waiting, so a held key doesn't pile up. Backward play plays the sound reversed.
- **D43. Enlarging a track** (pp. 647, 678). The enlarged track is three times its height, at least 180 px; in a short timeline the other tracks shrink first, to a line each. It is a view, like the zoom: not in the project or the undo history.
- **D44. An export's dynamic range and bit depth follow its footage** (§10; deviation X14). With no color management, `video.hdr` `auto` (the default) decides from the pictures an export shows, by duration. It is HDR when more than half of them are and the codec carries HDR (HEVC, AV1, VP9) with a 10-bit encoder here: HLG if every HDR frame is HLG, else PQ. Otherwise SDR, HDR tone-mapped (D35). An unset depth is 10 for HDR, and for SDR when most of the picture has 10 bits or more and the codec's 10 bits play widely (not H.264). The plan says why; the Export dialog notes HDR tone-mapped unasked, with an Export HDR button.
- **D45. Passes** (§10; Resolve's multi-pass option is on the Deliver page). Two passes bring an export closer to a target bitrate and spread its bits better; at constant quality they gain nothing worth the time. So an unset `video.passes` is two with a bitrate, where the encoder makes them, and one otherwise. Asking two of an encoder that makes one is an error.
- **D46. HDR metadata follows the footage** (§10; deviation X19). With no grade, an export carries what its sources say (`video.hdr_metadata` `auto`). PQ output gets the widest mastering display of the pictures shown and their highest MaxCLL and MaxFALL, measured where a source states none; SDR pictures count as BT.709 at 203 cd/m², HDR ones without a display as BT.2020 at 1000 cd/m². HLG and SDR output get none. Each frame keeps its HDR10+ and Dolby Vision where its transfer stays: HDR10+ (PQ only) when any picture has it, made from measured light for the rest; Dolby Vision when every picture has it, in one profile.
- **D47. Drop-frame timecode** (§4.3). At 29.97 and 59.94 fps record timecode can count drop-frame, as Resolve's project settings offer: frame numbers 00 and 01 (00–03 at 59.94) are skipped each minute but every tenth. New Project and Project Settings offer it; the first video setting the format (D37), or an OTIO import into an empty timeline, turns it on when the media's timecode counts so. It shows as `;` before the frames. Typed, `;` means drop-frame, as does `:` in a drop-frame project; a skipped label is refused, and digits that fill one land on the minute's first frame.
- **D48. Clips at another speed** (§1, §8.6; deviation X9). A clip's constant speed, from Change Clip Speed or an OTIO file, holds wherever its pictures and sound go. Change Clip Speed takes 1% to 10000% either way, or 0, freezing the frame under the playhead. Without ripple the clip keeps its length; with ripple it keeps its source, and later clips follow. Each frame shows the source frame its stretch of the source starts in (backward, its bottom, as Resolve shows it), without blending. Sound keeps its pitch, reversed backward; a freeze is silent. Edits count frames through the speed.
- **D49. What Join Match Edit Clips joins** (pp. 646, 950). The manual says only that it rejoins a split clip. Two clips join when the second plays on from where the first stops, so nothing seen or heard changes: adjacent on one track, with the same media, speed and settings, no frames missing, no fade or transition between. With two or more clips selected, those that meet join; otherwise those at the playhead, chosen as for Split (D2). The first part keeps its id.
- **D50. Dissolves** (§8.11; deviations X9, X24, X27). The manual says only that one picture fades as the next comes in; these rules match Resolve's renders. Frame i of an n-frame dissolve shows the incoming picture at (i + ½)/n and the outgoing at the rest, mixed in the output's code values, not linear light; at a gap, with what is under the track. A Cross Fade gives the incoming sound √x (+3 dB), x (0 dB) or x^1.5 (−3 dB) at x of the way, the outgoing the same of 1 − x, with no de-click at its edges. The manual names no track for the edit nearest the playhead, so it is Track 1's (D36). Ctrl+T on selected clips puts a dissolve on both edges of each, on its track, an edit between two of them once. A new dissolve is a second, centered with an odd length's middle frame before the edit and shortened to the media each side has (deviation X27), and adds a Cross Fade +3 dB where the sounds meet. Longer than both sides allow, it fills both, as in Resolve: 999 frames on a cut between two 300-frame clips with long handles make 300 + 300. At a clip's edge with no clip beside it, beside empty track or at the timeline start, it lies inside the clip, ending on its last frame or starting on its first, whether U picks the clip's side or both; the empty side alone takes none. A fade past the last clip, as an OTIO file may hold, lengthens the program.
- **D51. Loop** (§4.2; p. 640). The manual says only that Loop Playback loops. With it on, play that reaches the end goes on from the start; with both In and Out set in the viewer, play that reaches the Out goes on from the In, and play from past the Out runs to the end, then goes on from the In. Backward play loops the other way round. The sound runs on without a break. It is off at the start and not in the project.
- **D52. When the Trim Editor shows** (§8.6; pp. 729–731). The manual opens it with a double-click. Resolve Studio 21.1.1 shows it whenever an edit is selected and the player is stopped: a click on the edit, V or a double-click, or a click on a dissolve, which selects its edit and the dissolve. Either way an edit with a dissolve shows the dissolve. Space plays the timeline as usual, through the edit, the editor hidden; stopping brings it back, the edit still selected. Dragging its handles trims at the lower timeline's scale, a dissolve's ends at the filmstrips'.
- **D53. Import Media Folder** (§3; pp. 654, 658). The manual says each folder imported is a bin, and its bin sync expects bins named as the folders and the folders in them. Resolve Studio 21.1.1 makes a bin named as each folder in it, an empty one and one holding only documents too, and so does vidcut. Files and folders named with a dot stay out, and those Windows marks hidden come in, as in Resolve's Media Storage and its import. Resolve's order of the clips in the new bin changes from one import of the same folder to the next, and so does the video whose frame rate it offers a project with an empty pool: the first in that order. vidcut's media come in by name, ignoring case, a folder's files before the folders in it, so the first video in that order sets the format (D37). A file is media by its extension; the others, and media that don't open, are skipped, as in Resolve, but the result and the GUI's notice name them, and a folder without media is an error (deviation X32). A bin that exists takes the folder's media, and media already in the pool keep their ids and move into their folder's bin (deviation X31), so importing a folder again brings in only its new files, as importing into the bin that holds a file adds nothing in Resolve. Numbered images follow D39: `sequence`, or in the GUI a question. Checked on Resolve Studio 21.1.1's Cut page and through its scripting API, which has no call that makes the bins.
- **D54. Slow play with K** (§4.2; pp. 731, 871). The manual gives no speed, nor how long a tap lasts. Resolve Studio 21.1.1 plays at half speed with the sound slowed, in playback and in Dynamic Trim (checked by hand); whether it keeps the pitch, whether the playhead steps as J or L goes down, and how a held trim undoes were not checked. With K held, a J or L let go within 0.25 s is a tap: as it is let go, it steps a frame, or in Dynamic Trim trims one. Held longer, it plays that way at half speed, or in Dynamic Trim runs the trim at half speed, until J, L or K is let go; a run then applies as K applies it, as one undo step (D32). The sound plays slowed, its pitch kept, as at any speed but 1× (deviation X30). A held key's repeats are not presses, so holding L alone plays at 1×.
- **D55. Undo steps** (§8.10; pp. 651–652; deviations X4, X33–X38). The manual doesn't say what a step is or how it is named. Every request that changes the project is one step, an `apply` batch too; undo and redo aren't steps, and neither are views (zoom, enlarged tracks, the selection). Original is the oldest state the history keeps (X4). Resolve Studio 21.1.1 names a step by its kind ("Split Clips", "Remove clips", "Add Transition to Edit Points"); vidcut's names say more (X33). The steps a new change replaces stay as a version (X36), and a step can carry a name (X37).
- **D56. Subtitle tracks and cues** (§8.12). The manual doesn't say which track a new one shows or how long a new cue lasts. Subtitle tracks are `S1`, `S2`, … from the video up; a new one is the one shown. A new cue lasts 3 s, or until the track's next cue starts. Cues of one track may overlap and show together; on the timeline, cues that overlap share their track's height, each in a lane of its own. Split leaves its text in both parts; join keeps the first cue, to the last one's end, with their texts as lines. The viewer draws the shown track's cues as a burn-in draws them (D61), in the GUI's font.
- **D57. Cues follow Track 1** (§6, §8.12; p. 1242). The manual says only that subtitles linked to clips move with them. A cue keeps the frames of Track 1 under it: a ripple delete or trim takes out the part of a cue over the frames taken out, and drops a cue left with none; an insert inside a cue splits it, and both parts keep the text; overwriting and lifting leave cues where they are; a timeline In–Out delete takes its range out of cues too. A locked subtitle track doesn't move. Resolve Studio 21.1.1's ripple delete of a Track 1 clip (Edit page, through its scripting API) did the same: the cues inside it went, and a cue across its end kept its part after it.
- **D58. Subtitle file times** (§8.12; pp. 1230, 1247). Each time goes to the nearest frame. A file in timecode is timed in seconds from 00:00:00:00, counting the timeline's start timecode as frames at the project's rate: Resolve Studio 21.1.1 reads `01:00:00,000` on a 23.976 timeline as 00:59:56:10, and its renders write a cue at 01:00:00:00 as `01:00:03.599` at 23.976 and at 29.97 non-drop (through its scripting API; File > Export Subtitle not checked). A file read in is taken in real time, as Resolve writes and reads it, unless `time_base` on `subtitles.import` says labels (timecode read as a clock: 01:00:00:00 as `01:00:00,000`), or its times with fractions of a second fall on timecode frames and clearly off the frames of real time. Round times (whole seconds, tenths) count for neither, so a round `01:00:00,000` is 3600 s: 00:59:56:10 at 23.976, as Resolve reads it. At whole rates the two are the same. A subtitle file beside an export starts at zero at the output's start; beside an OpenTimelineIO file it is timed in that timeline's timecode, as Insert Selected Subtitles to Timeline Using Timecode needs (from an SRT in timecode it puts each cue where it was; from one timed from zero it inserts nothing; checked by hand). An SRT beside a timeline file has no `{\an8}` position tags, which Resolve shows as text.
- **D59. Sorting by timecode** (§3; pp. 666–667). The manual names the order, not how it sorts. Timecode sorts by the label of each file's start timecode (a video's recorded timecode, a broadcast WAV's time reference), hours, minutes, seconds and then frames as a part of a second, so files at different rates sort as their labels read: a 29.97 fps non-drop-frame clip at 01:00:00:00 (3.6 s past the hour in real time) comes before a 25 fps one at 01:00:02:00. A file without one counts as 00:00:00:00, the Start TC Resolve Studio 21.1.1 gives such a clip (checked through its scripting API), so it comes first ascending and last descending. Equal labels keep their import order, reversed when descending, as in the other orders.
- **D60. The Duration field** (§4.4; p. 693). The manual lists what the field takes, not how it reads it or what it counts from. A number alone is frames (`48`), not digits filling a timecode as in the timecode field (§4.3); a timecode, or a time with a unit (`00:00:02:00`, `2s`), is that long; `+` or `−` before either changes the duration shown by that much (`+12`, `-5`). The Out moves to the In plus the new duration. With no In, the duration counts from the clip's or the timeline's first frame, which becomes the In; with no Out, the duration shown runs to the clip's or the timeline's end. A duration under one frame is refused, as is a source Out past the media's end. `marks.set` and `media.marks.set` take the same as `duration`.
- **D61. Burned-in subtitles** (§8.12; p. 1247; deviation X45). The manual doesn't say how a track looks burned in before it has a style. In a burn-in render of Resolve Studio 21.1.1 over black (made through its scripting API, 3840×2160), a line's tallest letters are 4.2% of the picture's height, two lines 11.3%, both centered at 80.1% of it; its outline or shadow over a bright picture wasn't checked. vidcut draws white text 5.5% of the picture's height, lines 1.31 times that apart, with a black outline a twentieth of the text's height, centered and wrapped at 90% of the width; the block's middle is at 80% of the height, at 50% for a cue placed in the middle and 20% at the top. Measured at 1280×720 with Segoe UI: 4.2% and 11.3%, centered at 80.1%. The font is Open Sans where it is installed, else the system's sans-serif, with the system's fonts for scripts it lacks. Cues showing together at one place are one block, in the order they start. HDR white is 203 cd/m², graphics white in ITU-R BT.2408.
- **D62. Cues from speech** (§8.12; p. 1235). The manual names the options, not how speech becomes cues. A cue holds one sentence, or a part of one: it ends at a sentence's end and where the speech pauses for a second or more, and a sentence too long for one cue is split into cues of about equal length, at a comma where one is near. Words wrap onto the lines at the characters per line. A cue starts at its first word and ends at its last, and stays at least 5/6 s where the next cue allows; the frames between subtitles shorten a cue that would end closer to the next. Parakeet Ultra, the default, times each word. A model that gives no times, Qwen3-ASR until transcribe.cpp runs its aligner, has its text spread over the speech the voice-activity model found, by its characters, in pieces of 10 s at the most: its cues' times are estimates. Where the GPU in use has less memory than the model wants (Parakeet Ultra 2 GB, Whisper large-v3 4 GB, Qwen3-ASR 8 GB), the answer warns, and the model runs anyway; a model file the user names gets no warning.
