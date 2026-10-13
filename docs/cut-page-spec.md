# Cut page behavioral spec

vidcut copies the behavior of DaVinci Resolve's **Cut page** by default. This spec describes it in our own words, with page numbers from the *DaVinci Resolve 21.1 Reference Manual* (Cut page: Chapters 33–39, pp. 635–790; printed page = PDF page).
Differences: [deviations.md](deviations.md). §12 settles what the manual leaves open.

**The manual** is in `$VIDCUT_TEST_DATA` (default `~/Downloads/vidcut-tmp`); `pdftotext -f 635 -l 790 <manual>.pdf -` extracts the Cut page. Don't copy its text into the repo.

**Keys:** the manual's Command is Ctrl, Option is Alt. **Markers:** **(later)** not built yet; **(out)** out of scope (§1).

## 1. Scope

- **In:** media pool, viewer, dual timeline, editing and trimming, clip/track audio levels and waveforms, Quick Export plus our Export dialog.
- **Out (out):** effects, transitions, titles, generators, keyframes; color, Fusion, Smart Reframe, Close Up; speed changes, Sync Bin, Multi Source, Source Overwrite, Live Overwrite; Replay, voiceover, subtitles; AI tools (transcription, voice tools); Mixer FX, Paste Attributes, guides; DRT, AAF, XML.

## 2. Layout (pp. 636–637)

- Media Pool left, one Viewer right, timeline area below; the seam and the timeline height resize. Top bar: Media Pool, Mixer, Quick Export, Full Screen (Sync Bin, Transitions, Titles, Effects, Inspector: out). Edit-function buttons (§7) under the pool.
- Viewer: mode buttons, Duration field, transport bar (Previous Edit, Stop, Play, Next Edit, Loop, Mark In, Mark Out, timecode), audio meter with monitoring mute (p. 642).
- Timeline menus: Timeline Options (p. 645: Ripple On, Snap, Fixed Playhead, clip names/durations/status, Safe Edit, Trim to Audio, Boring Detector), Timeline Actions (p. 645: Add Track, Split Clips, Add Marker, Join Match Edit Clips), Edit Actions (p. 646: Trim Start/End to Playhead).
- Buttons: Ripple On, Split, Marker. Track-number buttons beside the upper timeline pick the target track, next to the Audio Only and Video Only toggles (pp. 643, 694).

## 3. Media Pool (Ch. 34, pp. 653–675)

- **Import** (p. 654): Ctrl+I, Import Media (button or right-click), multi-select; Import Media Folder. Frame rate: D37. File dialogs reopen where last used, across runs.
- **Stills** (D38) and **image sequences** (D39) (pp. 114, 366, 1194).
- **Remove** (p. 654): Delete or Backspace, confirmed; files on disk stay.
- **Views:** Thumbnail (p. 664): hover scrubs, I/O while hovering marks source In/Out, the lower-right corner shows metadata. List (p. 665): sortable columns. Filmstrip, Metadata cards (later).
- **Sort and search** (pp. 666–667): by date added (default), name, duration or file date, either way (Resolve: name, date imported or modified, timecode). The sort order is the Source Tape order. Search filters as you type, in the open bin and its bins.
- **Bins** (pp. 655–656, 377):
  - Master is the top level. The pool shows the open bin's clips and its bins as folders; a path (Master › Interviews) and the Bins menu's tree open any level, as does double-clicking a bin.
  - New Bin: File menu (Shift+Ctrl+N), Bins menu, right-click. Rename Bin, Remove Bin (clips stay, deviation X6). Move to Bin; Create Bin With Selected Clips ("Bin N", p. 656).
  - Dragging clips or a bin onto a bin or a path level moves them. A bin dropped on itself, inside itself or on its parent stays.
  - Imports go into the open bin (p. 377); Source Tape plays it and its bins (pp. 680, 696).
- **Clip color** (later) (pp. 667–668).
- **Relinking** (pp. 668–669): a missing file shows MISSING (red), its timeline clips are red (pp. 509, 530), the viewer shows Media Offline. Relink Media searches a folder tree; Relink… picks one file. Found files are checked (deviation X1).
- **Mouse** (pp. 680, 704, 719): click selects (Source Tape jumps to the clip); double-click opens Source Clip; drag to a timeline edits (§7.3). Shift-click, Ctrl-click, marquee, Ctrl+A.

## 4. Viewer (pp. 639–640, 679–693, 713)

### 4.1 Modes
- **Source Clip** (p. 680): the selected pool clip, with a scroll area for the whole clip (playhead, zoomed waveform, In/Out handles, Jog In/Out for one-frame nudges).
- **Source Tape** (pp. 680–681, 704–706): all pool clips end to end, in sort order; the pool selection follows the playhead and the other way round.
- **Timeline** (p. 681): the timeline at its playhead, no scroll area; icons mark a clip's first and last frame.
- Q toggles Timeline and the last source mode (p. 696); double-clicking a pool clip gives Source Clip; a click or drag in the timeline gives Timeline (p. 681).

### 4.2 Transport (pp. 640, 687–688, 870–872)
Space, J/K/L (pp. 696, 871), ←/→, Home/End, Shift+I/O (p. 870), Up/Down (p. 640): §11. Loop (Ctrl+/) and the jog control (later).

### 4.3 Timecode (pp. 640, 688, 713–714)
- `=` and a timecode goes there, `+`/`-` and a value jumps; or type in the field. Digits fill from the right (`100` is 1 s); `5s` and `450f` are durations.
- Right-click: source or record timecode or frame count; drop-frame (D47); Timer 1/2, KeyKode (later).

### 4.4 In/Out (pp. 691–693, 700)
- Source: I/O in a source mode or over a pool thumbnail, or drag the handles.
- Timeline: I/O in Timeline mode or while dragging a ruler; shown and draggable on both rulers.
- Alt+X clears both. The Duration field shows the source duration; editing it (frames, ±frames, timecode) moves the Out (p. 693).

### 4.5 Other viewer features
- Full Screen (Shift+F), Enhanced viewer (Alt+F), safe areas, Fast Review, skimming with C (later).
- **Audio scrubbing** (p. 876; on by default; Shift+S or Timeline > Audio Scrubbing): dragging the playhead and frame steps play the sound there (D42).

## 5. Dual timeline (pp. 643–646, 689–692, 716–718)

- **Upper timeline:** the whole program, no zoom; its playhead moves freely and its ruler scrubs. Most edits work there, clips drag between the timelines, and it shows the In/Out and markers.
- **Lower timeline:** fixed zoom around the playhead (p. 644). Fixed Playhead (default, D1): the playhead stays centered and the ruler drags the clips, which don't snap to it (p. 717). Off: the playhead moves (click the ruler or drag it), paging at an edge.
- **Tracks:** video+audio 1, 2, …; audio-only A1, A2, … below. Video clips show a filmstrip with the waveform along the bottom, larger when enlarged (p. 647). Toggles: names, durations (seconds:frames), status icons. Locked track: stippled; disabled track: dimmed; disabled clip: dull; muted clip: mute icon.
- **Headers** (pp. 647, 678, 717): Enlarge, then Lock, Mute and Video Enable icons (D43).
- **Boring Detector** (p. 690): marks clips longer than Boring Clips (gray) and shorter than Jump Cuts (red) on the upper timeline; Analyze Timeline Edits sets both (D31).
- **Detect Scene Cuts** (p. 690): splits the selected clips at shot changes (D30); Studio-only in Resolve, in every vidcut build.

## 6. Track model (pp. 647, 649, 716, 721, 727)

- **Track 1 is the main story:** insert, delete, move and trim ripple, so it has no gaps. Ripple On off makes it like other tracks; Alt while trimming suspends rippling.
- **Tracks 2+:** no ripple of their own, gaps allowed. A clip follows the Track 1 clip under its In (p. 649; D21) and is deleted when that clip's Out is trimmed before it (p. 727). They ripple only where no Track 1 video is under them (p. 721; D22).
- **Audio tracks** hold sound alone (D26).
- **Header** (p. 647): target toggle (D3), lock, mute, video on/off; the target track is lighter. Right-click adds or removes tracks (D27).
- The highest enabled track's picture shows (p. 647), skipping disabled clips and tracks with video off. Every audible track is mixed (§9).
- A clip's video and audio are one clip; trims move both, except a J/L cut (§8.7). No link/unlink.

## 7. Edit functions (pp. 696–701)

Pick a source, mark source In/Out, optionally pick the target track, press an edit key, Q to review and back (p. 696). Video Only and Audio Only limit what an edit takes (p. 694).

### 7.1 Smart Indicator (p. 696)
Marks the edit nearest the playhead, which some functions use instead of the playhead: green, or red where the clip has no media beyond it; not animated (deviation X15).

### 7.2 Edit functions
- **Smart Insert** (F9; p. 697): at the Smart Indicator's edit on the target track; on Track 1 later clips move right.
- **Append** (Shift+F12; p. 697): after the last clip.
- **Ripple Overwrite** (Shift+F10; p. 698): replaces the clip under the playhead, or the timeline In/Out range; the source duration wins and later clips shift by the difference.
- **Overwrite** (F10; p. 701): on the target track at the playhead, the timeline In, or backtimed from a lone Out; nothing else moves.
- **Place On Top** (F12; pp. 699–700): on the first free track above (D24), never rippling. Aligns timeline In to source In; else a lone timeline Out to source Out; else, with no marks, an edit within 5 frames; else the playhead.
- Close Up, Source Overwrite (out).

### 7.3 Drag and drop from the pool or Viewer (pp. 648, 679, 694–695)
- Empty timeline or left of the first clip: first clip. Past either end: appended or prepended. On an edit point: inserted.
- On a clip, released at once: Ripple Overwrite replaces it. After a hover (D11, D18): Overwrite, which may split it.
- Above the top track: a new video+audio track; below the bottom, sound makes an audio track.
- Another track: overwrite, never ripple (D23). Sound on a video track goes to the first audio track.

## 8. Timeline editing (Ch. 36, pp. 715–732)

### 8.1 Selection (pp. 719–720)
- Clips: click, Ctrl+click, Shift+click, marquee; empty space or Ctrl+Shift+A deselects; Alt+Shift+V selects the clips under the playhead.
- Edits: the center selects both sides (roll), beside it one side (resize); Ctrl+click adds; V selects the nearest edit, U cycles outgoing / incoming / both.

### 8.2 Moving clips (pp. 720–723)
- Onto a Track 1 clip, released quickly: replaces it and ripples. After hovering: overwrites, splitting it. Track 1 closes the gap left behind.
- Over a neighbor's edge (p. 722): the overlap is overwritten and the gap closes, shortening the neighbor.
- Onto an edit point: inserts there; later clips shift with their attached clips.
- A Track 1 clip dropped on another track overwrites there, and Track 1 closes up (p. 721). Higher-track clips move freely (D25).

### 8.3 Copy, cut, paste (p. 723)
Ctrl+C, Ctrl+X (ripples closed), Ctrl+V (at the playhead on the original track, overwriting).

### 8.4 Delete (pp. 721, 724–725)
Delete removes the selection (rippling on Track 1 with Ripple On), or the timeline In/Out range when set. D toggles a clip's video; right-click > Mute silences its audio.

### 8.5 Split (pp. 646, 723, 950)
Ctrl+\, the Split button or Split Clips (D2). Join Match Edit Clips (later).

### 8.6 Trimming (pp. 693, 716, 718, 726–732)
- Clip edge: resize, with a tooltip of the delta and new duration. Edit center: roll, with a white overlay of the handles. Clip center: slip, of all selected clips. Lower audio band at an edit: a J/L cut (§8.7).
- **Safe Edit** (p. 732): a trim stops at the neighbor, with a timer; dragging on overwrites. **Trim to Audio** (later).
- **Trim Editor** (double-click an edit; pp. 729–730): outgoing over incoming filmstrip; top handle trims the Out, bottom the In, middle rolls; ±1-frame buttons. vidcut shows the two frames at the cut; draggable filmstrips (later).
- **Trim Start/End to Playhead** (p. 729; D4).
- **Change Clip Duration** (Ctrl+D, p. 693): frames, timecode or 1/5/15 s, on every selected clip; End extends to the media's end; Extend Beyond Clip Length (later).
- **Dynamic Trim** (W, pp. 730–732): the playhead turns yellow and J/K/L trim the selected edit (side by U, else the nearest edit) or slip or slide the selected clips (right-click the button); Space plays around, Up/Down change edit (D32, deviation X11).
- **Slide** (p. 731): moves a clip over its media, lengthening the clip before and shortening the one after.

### 8.7 J/L cuts (p. 728)
Hover a clip's audio at an edit until the cursor shows a note, then drag: only the audio edit moves. A click selects it for `,`/`.` (D20).

### 8.8 Snapping (pp. 716–717)
N toggles snapping between clip edges, markers, playhead and In/Out, shown by a white line; mid-drag the toggle is temporary.

### 8.9 Markers (pp. 718–719)
M adds or edits the marker at the playhead; Shift+Up/Down jump between markers; markers drag; Alt+M deletes the one at the playhead, Delete the selected ones.

### 8.10 Undo (pp. 651–652)
Ctrl+Z / Ctrl+Shift+Z, unlimited; the History list jumps back; a new change drops the redo steps.

## 9. Audio (pp. 642, 649–650, 743, 771, 773)

- **Meters** (D28): green safe, yellow near the limit, red risks clipping. The viewer's meter shows what it plays; its speaker mutes playback, not export (p. 642).
- **Mixer** (pp. 649–650): per track with sound, a strip with name, meter, dB fader, mute and solo, plus the mix's meter. Mute and solo apply to export (p. 650). A fader is heard while dragged and saved on release. Pan and FX (out).
- **Clips:** one gain value (D12) and mute.
- **Ripple Delete Silence** (p. 773; vidcut: the clip's menu or Timeline Actions): threshold, pre-head, post-tail and minimum; the parts to remove show as red stripes, and Remove cuts them and closes the gaps (D29; crossfade: deviation X11).

## 10. Quick Export (Ch. 39, pp. 788–790, 4095–4096, 4104–4109)

- File > Quick Export or the toolbar button, a preset, Export, a file name, progress. Range: the timeline In–Out, else the whole timeline. One file (per-clip: our Export dialog).
- Presets after the Deliver page's: H.264 Master (QuickTime, AAC 320 kb/s), H.265 Master (Main10), ProRes 422 HQ (LPCM), YouTube 720p–2160p (MP4, H.264 High), Vimeo, TikTok (1080×1920). User presets can join the row.
- Quick Export row, Export dialog, jobs: deviation X3. The suggested name is the project's. Interchange: deviation X9.

## 11. Keyboard shortcuts (Linux and Windows)

† marks Edit-page keys (D13). Keys do nothing while a text field has focus or a modal question is up; Esc dismisses the question without acting.

| Key | Action | Page |
|---|---|---|
| Space | Play / stop | 640 |
| J / K / L | Shuttle reverse / stop / forward (repeat = faster) | 696, 871 |
| K+J / K+L; K held + tap J/L | Slow play; one-frame step | 871† |
| ← / → (Shift: one second) | Step one frame | 870† |
| Up / Down (Shift: markers) | Previous / next edit | 640, 689, 718 |
| Home / End | Timeline start / end | 870† |
| I / O (Shift: go to) | Mark In / Out (source or timeline, by context) | 691, 870† |
| Alt+X | Clear In and Out | 700 |
| = / + / − | Go to timecode / relative jump | 688 |
| Q | Viewer: Timeline ↔ last source mode | 696 |
| F9 / F10 / Shift+F10 | Smart Insert / Overwrite / Ripple Overwrite | 889†, 701, 890† |
| F12 / Shift+F12 | Place On Top / Append | 890† |
| Ctrl+\ or Ctrl+B | Split at playhead (Ctrl+B needs no AltGr) | 723 |
| Delete / Backspace; Alt+Delete | Delete (ripples on Track 1); lift (D9) | 725 |
| Ctrl+C / X / V | Copy / cut (ripple) / paste at playhead | 723 |
| Ctrl+A / Ctrl+Shift+A; Alt+Shift+V | Select all / none; clips under the playhead | 719 |
| V / U | Select nearest edit / cycle edit side | 720 |
| , / . (Shift: one second) | Trim the selected edit a frame left / right | 870† |
| D / Ctrl+D | Disable clip video / change clip duration | 724, 693 |
| N / Shift+S | Snapping / audio scrubbing on/off | 716, 876 |
| M / Alt+M | Add-or-edit / delete marker at playhead | 718 |
| W | Dynamic Trim on/off | 731 |
| Ctrl+mouse wheel | Zoom the lower timeline | 644 |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo | 651 |
| Ctrl+I / Shift+Ctrl+N | Import media / new bin | 654, 655 |
| Alt+F / Shift+F / Esc | Enhanced viewer / full screen / exit (later) | 688, 40† |

## 12. Decisions on gaps and contradictions

A decision holds until the manual or a test shows Resolve does otherwise.

- **D1. Fixed playhead:** the lower timeline's is fixed by default, with a toggle (fixed on p. 689; a toggle on pp. 645, 716).
- **D2. What Split cuts:** the selected clips under the playhead, if any, else every clip under it on unlocked tracks; the left halves end up selected (the Edit page's Auto Select, simplified; p. 950).
- **D3. Target track:** Smart Insert, Ripple Overwrite and Overwrite use it (V1 by default; A1 for sound alone, D26). Append always uses Track 1; Place On Top picks its own (D24).
- **D4. Trim Start/End to Playhead** applies to the selected clips, else the Track 1 clip under the playhead.
- **D5. Missing source marks:** no In is the first frame, no Out the last. In Source Tape with no marks, the whole clip under the playhead.
- **D6. Marks persist after edits:** source marks per clip, timeline marks until Alt+X.
- **D7. Overwrite with timeline In and Out:** a three-point edit from the source In; too little source media is an error.
- **D8. Ripple Overwrite with timeline In and Out:** the source duration wins; the program ripples by the difference (p. 698).
- **D9. Lift on Track 1:** with Ripple On off, Delete leaves a gap; Alt+Delete lifts once.
- **D10. Overwrite past the end of Track 1:** with Ripple On it starts at Track 1's end, keeping Track 1 gapless.
- **D11. Hover time:** 0.8 s turns a drop or move onto a clip from replace into overwrite.
- **D12. Clip gain range:** −60 dB to +24 dB, plus mute.
- **D13. Keys** the Cut chapters lack come from the Edit page († in §11).
- **D14. J/L cuts belong to an edit point,** so audio never overlaps on a track or reaches into gaps.
  - A J/L cut is one offset on both clips: the outgoing `tail` equals the incoming `head`.
  - An edit that gives a clip a new neighbor there (delete, move, split inside the overlap) makes it a straight cut.
  - A clip heard only past its picture (a neighbor's J/L cut covers it) loses both its cuts when its own resets.
- **D15. Playhead after an edit function** (Edit page): after Smart Insert, Append, Overwrite, Ripple Overwrite or Paste, scripted too, it moves to the first frame after the new clips, and the Viewer keeps its mode for Q (p. 696). A drop leaves it.
- **D16. Mark Out includes the frame under the playhead:** ranges are half-open, so O at frame `f` stores `out = f + 1`, as JSON shows.
- **D17. Source Tape In/Out** (p. 692) mark the whole tape, stored as tape frames with the clip order, so an edit takes the same span whatever the pool shows later.
  - A span within one clip is also that clip's source marks (D5), cleared together; clearing a span across clips leaves their own marks alone.
  - An edit given the tape takes the span (first clip's tail, the clips between, last one's head) as one block with one clip's rules (insert, Ripple Overwrite, Place On Top, D7), in one undo step.
  - Removing a tape clip, or relinking one to a file of another length, clears the tape marks.
- **D18. Drop and move targets.** Within 6 px of an edit point (not the moved clips' own): insert. On a clip: replace within 0.8 s (D11), else overwrite; several clips always overwrite. Past the last clip: to the end; before the first: the start.
  - The row under the pointer is the track; the strip above makes a video track, up to 24 px below an audio track. Further off, or Esc, cancels.
- **D19. Trimming with the pointer:** edges follow the pointer's movement from the press, snapping (§8.8). A trim, roll, slip or J/L cut stops where the media runs out. Safe Edit holds 0.6 s at the neighbor (p. 732). Dragging empty space scrubs (upper timeline) or draws a marquee (lower).
- **D20. Keyboard trimming:** `,`/`.` trim the selected edit (V, U or a click) a frame, with Shift a second: Out, In or both. These are Edit-page keys; the Cut page's is Dynamic Trim (D32).
  - The J/L handle's click selects the audio edit (D36); `,`/`.` move both clips' `tail`/`head`, leaving the picture cut. U leaves an audio edit alone.
- **D21. Following Track 1** (pp. 649, 727). A clip on another track is attached to the Track 1 picture under its first frame (clip and source frame), worked out at each edit.
  - It moves when that picture moves (shift, move, even to another track); split parts count as the same picture.
  - Its clip's tail trimmed past it: deleted (p. 727); head trimmed past it: moved to the new start; replaced: stays; deleted: moves where Track 1 closed up.
  - Over a gap or past Track 1's end, it keeps its distance to the next Track 1 clip (or from the one before).
  - Locked tracks don't follow. A clip that would land on another moves to the first free unlocked track of its kind (up; audio: down), created if needed.
- **D22. Rippling other tracks** (p. 721): with Ripple On, a delete or trim there ripples only when Track 1 has no clips from that clip on; else it leaves a gap. Deleting a timeline In–Out range cuts it from every unlocked track, closing them together (p. 725).
- **D23. Edit functions on other tracks:** only Track 1 pushes (p. 697). Elsewhere inserts overwrite, and Ripple Overwrite moves nothing else. Paste returns each clip to its own track, created if missing (p. 723). Split: D2.
- **D24. Place On Top** (pp. 699–700) goes on the video track above the highest one with clips in the span, created if needed, or Track 1 if empty there (D10). Sound alone: the first audio track with room, or a new one.
- **D25. Moving clips between tracks:** positions are given as before the move. A Track 1 clip moved off lifts out; with Ripple On, Track 1 closes up and the clips land where dropped. Onto Track 1, clips insert or overwrite (§8.2). Video can't go onto an audio track; a multi-track selection stays on its tracks.
- **D26. Sound alone** (no video, or Audio Only, p. 694) goes to A1 unless a track is given; a clip on an audio track keeps only its audio.
- **D27. Track numbers are positions:** V1, V2, … from the bottom, A1, A2, … from the top; removing renumbers. Naming the next track creates it. Track 1 can't be removed; a track with clips needs confirming. Track volume: −60 to +24 dB.
- **D28. Meters:** a bar per project channel (p. 649), linear peaks from −60 dBFS to 0, held 1.5 s.
  - They show the edit's levels at the playhead, playing or paused, not the player's output ([todo](todo/meters-tap-player-output.md)): 10 ms peaks with gain, fader, mute and solo as in export (p. 650), mixed to the project's channels like export. The mix can pass full scale.
  - The monitoring mute silences only the viewer meter.
- **D29. Ripple Delete Silence** (p. 773). Silence: every channel's peak, after clip gain, under the threshold, in 10 ms steps; no audio counts as silent. Defaults: −40 dBFS; pre-head and post-tail 0.15 s; minimum 0.5 s.
  - A silence at least the minimum goes, less the post-tail after the sound before it and the pre-head before the sound after; at a clip's ends it goes entirely. Whole frames only.
  - It ripples whatever Ripple On says: Track 1 closes up and other tracks follow (D21). One undo step; a dry run lists the parts.
- **D30. Finding shots and black** (p. 690): a frame whose scene score (FFmpeg scdet's, 0–100) is 10 or more starts a shot, split at the first timeline frame showing it. Black: 98% of pixels within 10% of black, for 0.5 s or more (`analyze.black`).
- **D31. Boring Detector defaults** (p. 690): boring over 30 s, jump cuts under 10 frames (2 finds flash frames), on video tracks. The settings are view options, not in the project.
- **D32. Dynamic Trim** (pp. 730–732).
  - J/L run the trim at shuttle speed (repeat: 2×, 4×, 8×) until the media or a neighbor runs out; the cut turns red and the viewer says why.
  - The viewer shows the frames on both sides as the trim leaves them; the timeline redraws during the run. The selected edit follows; an edit or undo that removes it clears it.
  - K applies the run as one edit; K held with J/L tapped applies a frame; W applies what's pending. Space plays from 2 s before to 2 s after.
- **D33. Rotation metadata:** a display-matrix rotation (phones' portrait video) applies everywhere pictures show; a portrait clip that sets the format (D37) makes a portrait project. `media.get` reports the shown size and the rotation. Mirroring is ignored.
- **D34. Interlaced sources** (p. 741): interlaced by field order or first frame (`media.get` `fields`). Frames flagged interlaced are deinterlaced everywhere, one frame per frame from the first field (deviation X13). Output is never interlaced; at twice the clip's rate each frame shows twice.
- **D35. HDR and color** (pp. 230–239). PQ or HLG clips are HDR (`media.get` `hdr`).
  - SDR output tone-maps them to BT.709: up to 80% of 203-nit white unchanged, highlights rolled off up to 1000 nits. HDR output (D44) is BT.2100 10-bit, SDR white at 203 nits (BT.2408; p. 238).
  - Untagged media are BT.709 at 1280 wide or over 576 lines, else BT.601. Outputs are tagged. The viewer uses the export's math (deviation X14).
- **D36. Which track an edit point is on** (pp. 696, 720, 729–732). The Smart Indicator, V, Smart Insert, Place On Top and Up/Down use Track 1's edits.
  - A clicked edge, cut or J/L handle selects that track's edit (the handle: its audio edit, D20). `,`/`.`, U, the Trim Editor and Dynamic Trim (its Up/Down too) then stay on that track, an audio selection on audio edits.
  - `,`/`.` do nothing during a Dynamic Trim run.
- **D37. The project format.** In a project created without one, the first media with video sets the rate and size as shown (D33), without the prompt Resolve shows (p. 654), which would stop scripts.
  - New Project takes any even size from 16 px and any positive rate.
  - The size changes any time (clips are fitted when drawn); the rate only while the timeline is empty (positions are whole frames).
- **D38. Stills** (pp. 114, 1194). The manual says 5 s (p. 114) and 10 s (p. 1194); vidcut uses the project's still duration, 5 s by default (deviation X16), when no range or marks are given.
  - A range or trim can make it up to 17 h 40 min (p. 1194). A still has no frame rate and doesn't set the project format; the first video does (D37).
  - Images are what FFmpeg reads as images.
- **D39. Image sequences** (p. 366): with `sequence`, numbered files (`frame_000001.png`, …; padded or not), the unbroken run around each file given, come in as one clip at the project's rate unless `fps` is given, named `frame_[000001-000120].png`. Without it, each is a still (deviation X17).
- **D40. Timeline interchange with Resolve** (deviation X9; pp. 527–530, 4160), following Resolve's own files:
  - Source ranges count from the media's start timecode (else 0).
  - `target_url` is a plain path (relative in bundles); Resolve misses `file://` URLs and `%20`. vidcut reads both.
  - Audio stays with its video by Resolve's link group number.
  - Marker notes go in `Resolve_OTIO.Note` and `comment`; a marker's length (one frame by default) is its marked range.
  - Track names: deviation X9. Resolve writes no timeline name and keeps none on import.
  - `.otioz` as OTIO's adapter writes it, `.otiod` as a folder; a bundle unpacks into a folder named after it.
- **D41. Proxies** (deviation X18; pp. 207–218).
  - For videos and image sequences, on demand (Generate Proxy). `proxies.codec` `auto`: AV1 on a hardware encoder, else H.264; without x264 and SVT-AV1, OpenH264, else ProRes Proxy (10-bit 4:2:2). Longest side `proxies.size` (960). Files in `proxies/` beside the project.
  - With `proxies.auto` and `use_proxies` on, the GUI makes them for image sequences, videos over 3840×2160 and files over 150 Mb/s, when imported or when `use_proxies` turns on. Headless runs never do.
  - `use_proxies` (in the project) plays them in the preview only; preview audio, export, frames, analysis and OTIO use the originals. A proxy whose source's size or time changed isn't played; relinking to the same file keeps it.
- **D42. Audio scrubbing** (p. 876).
  - A frame step or a paused playhead's move plays that frame's sound from the mix, faded over 3 ms, always forward (a 33–42 ms frame isn't heard as backward). The monitoring mute silences it; while playing, nothing extra plays.
  - A frame's sound plays whole; a newer one waits, replacing any waiting.
  - Playing backward (J) reverses the sound, its pitch kept at other speeds.
- **D43. Enlarging a track** (pp. 647, 678): one track at a time, three times its height and at least 180 px, others shrinking to one line if needed. A view only: not saved or undone. Header icons show state and are lit when a track is locked, muted, video off or enlarged.
- **D44. Export dynamic range and bit depth follow the footage** (§10; deviation X14), from the visible pictures in the range, weighted by duration (per file in a per-clip export); gaps don't count.
  - `video.hdr` `auto`: HDR when over half the picture is HDR and the codec carries it (HEVC, AV1, VP9) on a 10-bit encoder here; HLG if all HDR frames are, else PQ. Else SDR (D35). `sdr`, `pq`, `hlg` set it.
  - Unset `video.bit_depth`: 10 for HDR; for SDR 10 when over half the picture is 10-bit and the codec's 10-bit form plays widely (not H.264), if the encoder an 8-bit export would use writes it. ProRes 10; DNxHR its profile's (HQ becomes HQX). A set depth, or a profile that fixes one, wins.
  - Presets use `auto`; TikTok uses `sdr`. The plan and result say what and why.
  - When HDR will be tone-mapped and SDR wasn't asked for, the plan notes the overrides that keep it HDR, and the Export dialog offers them as Export HDR in one dim line. It never asks.
- **D45. Passes** (§10; Deliver page). `video.passes` is 1 or 2; unset, 2 with a target bitrate when the encoder makes two, else 1. Two need a bitrate.
  - x264, x265 and VP9 encode twice; NVENC looks twice in one encode. Other encoders make one ([todo](todo/svt-av1-two-passes.md)); asking for two is an error, and `auto` picks one that can.
  - Defaults: `first_pass` `fast` (or `full`); NVENC `multipass` `quarter` (or `full`), `lookahead` 32 (0–32). Cancelling leaves nothing.
- **D46. HDR metadata follows the footage** (§10; deviation X19; p. 4117), as read when media are added.
  - `video.hdr_metadata` `auto`, PQ output only: the widest mastering display and the highest MaxCLL and MaxFALL of the pictures shown (SDR as BT.709 at 203 cd/m², HLG and PQ without a mastering display as BT.2020 at 1000). `video.mastering_display` and `video.max_cll` set them.
  - Missing light levels are measured from the shown frames when the export starts (`measure` measures all).
  - Each output frame keeps its source frame's dynamic metadata where the transfer stays. HDR10+ (PQ) is kept when any picture has it, made for the frames without. Dolby Vision is kept when all have it (8.1 into PQ, 8.4 into HLG, 10 for AV1), on a CPU encoder.
  - `static`, `hdr10plus` (made for every frame if none has it), `dolby_vision`, `all`, `none` choose; the plan, result and Export dialog say what and why.
- **D47. Drop-frame timecode** (§4.3). At 29.97 and 59.94 fps a project can count drop-frame: 00–01 (00–03 at 59.94) skipped each minute but every tenth, so `01:00:00;00` is frame 107892.
  - Set in New Project or Project Settings; refused at other rates, and off after a change to one.
  - Shown with `;` before the frames everywhere; typed `;` means drop-frame, and `:` does too in a drop-frame project. A skipped label is refused. A media file's own timecode shows as that file counts it.
  - Turned on when the media's timecode is drop-frame, for the first video that sets the format (D37) or an OTIO import into an empty timeline.
