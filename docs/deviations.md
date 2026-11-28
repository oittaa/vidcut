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
  - A project is one `.vidcut` SQLite database file. It holds both the project and its undo history. There is no library of projects.
  - The project file stores the paths of media in or under the project's folder relative to that folder.
  - Relink Media finds missing files by name. It relinks only the same file ([design/model.md](design/model.md#fingerprints-and-relinking)).
- **Why:**
  - Scripts and agents edit projects through the CLI while the GUI can have them open. One file keeps all of them in agreement.
  - SQLite gives atomic incremental saves and keeps the undo history with the project when the file moves. Standard SQLite tools can read or edit the file.
  - The user can move the project folder, also to another computer, and keep its media.
  - A different file with the same name does not take the place of the media.

## X2. TikTok is one size

- **Resolve:**
  - The Quick Export row of Resolve Studio 21.2 has TikTok.
  - TikTok has a menu of two resolutions (p. 4106). The manual does not name them.
- **vidcut:**
  - TikTok is one preset, 1080×1920, with no menu.
- **Why:**
  - The two TikTok resolutions are not measured yet.

## X3. OpenTimelineIO interchange

- **Resolve:**
  - Timelines move between applications as EDL, AAF, XML or OTIO files. File > Import Timeline and File > Export do this, off the Cut page (pp. 527–530, 4159–4160).
- **vidcut:**
  - vidcut uses only OpenTimelineIO, for import and for export, in the shapes that Resolve uses ([D40](spec.md#12-decisions-on-gaps-and-contradictions)).
  - File > Export Timeline is `export start cut.otio`. With Media, the export is a bundle ([design/model.md](design/model.md#opentimelineio)).
  - An OTIO file also opens as a new project.
  - OTIO has no drop-frame flag. An imported timeline takes the drop-frame setting of the project.
  - A project that had no timeline before the import also turns drop-frame on when the timecode of the media is drop-frame.
  - An import into a project with no media turns drop-frame off.
  - Clip effects, transitions, generators that vidcut does not draw, compound clips and speed changes come in as data. vidcut writes them back while they fit.
  - Dissolves, cross fades ([D50](spec.md#12-decisions-on-gaps-and-contradictions)), wipes and speeds ([D48](spec.md#12-decisions-on-gaps-and-contradictions)) play.
  - The generators of [spec §8.17](spec.md#817-generators-p-766) play.
  - Other transitions play as cuts, and other generators play as gaps. Other effects do not play.
  - The Text and lower-third titles of Resolve come in as titles of vidcut. The titles of vidcut go out in the shape that Resolve uses ([D67](spec.md#12-decisions-on-gaps-and-contradictions)).
  - The Transform, Cropping, Dynamic Zoom, Composite and Retime and Scaling of Resolve come in as the picture of a clip. The picture of a clip goes out as these effects ([D75](spec.md#12-decisions-on-gaps-and-contradictions)).
  - Resolve Studio 21.1.1 imports `.otio` and `.otioz` files. It imports an `.otiod` folder by its `content.otio`.
  - It keeps tracks and their names, link groups, J/L cuts, clip gain, markers, stills, image sequences and the timecode of sources.
  - It also keeps clip effects and their keyframes, transitions, generators, speeds, and compound clips with their picture and sound linked.
- **Why:**
  - OTIO is an open standard that Resolve and other editors read and write. Thus a cut can go to a finishing tool and come back.
  - The EDL of Resolve keeps one video track, and no audio tracks or markers. Its FCPXML (1.10) keeps no markers.
  - vidcut carries what it does not model. Thus a cut that is finished in Resolve can come back for a recut and go back intact.

## X4. The timeline color space has no grading

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
- **Why:**
  - With Auto, mixed phone, camera and screen footage comes out correctly without color settings. The HDR video of a phone stays HDR.
  - The working space of Resolve's color management is not known. A straight line in the code values is what Resolve does without color management.

## X5. Image sequences are asked for, not guessed

- **Resolve:**
  - The Frame Display Mode decides whether numbered images are one sequence or stills.
  - Auto decides by file type: DPX and EXR files come in as one clip, and JPG files as stills (p. 366).
- **vidcut:**
  - Each image is a still, unless `media add --sequence` asks for the numbered run around it.
  - `media add-folder --sequence` asks for the runs in a folder ([D39](spec.md#12-decisions-on-gaps-and-contradictions)).
  - When the user imports numbered images in the GUI, picked or in a folder, vidcut asks which they are.
  - The question has one clip for each run already selected, for every image type.
- **Why:**
  - The file type does not tell the frames of a render from numbered photos.
  - A wrong guess turns a slideshow into a blink, or a render into thousands of stills.
  - A flag keeps scripts predictable.

## X6. HDR metadata comes from the sources

- **Resolve:**
  - An HDR export carries the metadata of the grading setup: the Mastering Display and the HDR10 metadata (p. 4117).
  - It also carries Dolby Vision and HDR10+ from the grade (pp. 151, 259–260).
  - Resolve Studio measures MaxCLL and MaxFALL (p. 282).
- **vidcut:**
  - An export carries what its sources tell, or measures it from their frames ([D46](spec.md#12-decisions-on-gaps-and-contradictions)).
  - With a grade or a LUT, an export measures MaxCLL and MaxFALL on its own frames. It takes nothing from the sources.
- **Why:**
  - Phone, camera and graded HDR footage looks on TVs as its sources do.
  - This needs no setup by a colorist.

## X7. One folder for proxies and the render cache is a preference

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

## X8. Slowed sound keeps its pitch on every system

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

## X9. A file is in the pool once

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

## X10. Subtitle files placed by their timecode, or from the timeline's start

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

## X11. A scroll rises over the whole clip

- **Resolve:**
  - Scroll moves its text from the bottom of the picture to the top. The length of the clip sets the speed (p. 1178).
  - These are measurements in Resolve Studio 21.1.1:
    - The middle of the first line is on the bottom edge one frame before the clip starts.
    - The text rises at a steady speed.
    - How far it goes past the height of the frame changes with the text size and the picture size. It is 1.9 to 2.9 times the text size.
    - Thus the text leaves the top before the clip ends.
- **vidcut:**
  - A `scroll` title rises at a steady speed.
  - It starts just below the picture at the first frame of the clip. It ends just above the picture at the last frame.
- **Why:**
  - Credits need a scroll that uses the full clip, and that starts and ends clear of the picture.
  - The distance in Resolve cannot be found from the size of the text and of the picture.

## X12. Create Subtitles from Audio reads each clip's file

- **Resolve:**
  - Create Subtitles from Audio analyzes the speech in the timeline.
  - It "commonly fails" where dialog overlaps on several tracks. The manual says to mute the tracks that are not wanted (p. 1235).
  - A timeline transcription, in the Transcription window, transcribes the media of each clip through its In and Out instead (p. 1001).
- **vidcut:**
  - By default (`hear: media`), `subtitles.transcribe` reads the words of the file of each heard clip.
  - It uses the words kept with the file when the same model made them in the requested language. Otherwise it transcribes the whole file and keeps its words.
  - It places the words through the range and the speed of each sound clip.
  - Two tracks can have the same word at the same time. Then it keeps the word of the first track (A1, A2, …).
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

## X13. The lower timeline keeps its height, and its tracks scroll

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

## X14. Speakers are numbered per file or per run, and joined by hand

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

## X15. Files and folders dropped on the window go into the open bin

- **Resolve:**
  - Clips dragged from the file system go into the Media Pool. A drop on a bin in the Bin list puts them in that bin (p. 364).
  - For the Cut, Edit, Fusion and Fairlight pages, the manual says macOS only (p. 372).
  - Folders dragged from the file system into the Bin list come in as bins, nested as they were (p. 365).
  - The manual does not tell what a folder dropped on the Media Pool of the Cut page does.
- **vidcut:**
  - With a project open, files and folders dropped anywhere on the window go into the open bin.
  - The files come in as Import Media brings them in.
  - Each folder comes in as Import Media Folder brings it in: a bin, with a bin for each folder in it.
  - While they are over the window, the Media tab shows Drop to Import.
  - With no project open, a drop does nothing, because Import is not available then.
- **Why:**
  - The window library, winit 0.30, gives no place with a dropped file. Thus a drop cannot aim at a bin or the timeline. The open bin is where Import puts media.
  - A folder keeps its folders as bins, as the Bin list of the manual does. The Cut page has a Bin dropdown, not a Bin list (p. 655).

## X16. Ducking follows speech by default, with one fall shape

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
  - A new ducking has every other audio track as its sources.
  - `track.set` refuses a value outside its range.
- **Why:**
  - Room noise, breaths, bumps and music on a dialogue track are louder than a fixed threshold. A speech detector tells speech apart from them.
  - Sound level keeps the behavior of the Ducker, for sound effects and music as sources.
  - One fall shape for every Rise Time is easier to predict. A start on a grid of 10 ms is of no use to the user.
  - The usual mix ducks music under every other track. A value that changes without a word is a different setting than the one asked for.

## X17. The Inspector is a tab beside the Media tab, in frame pixels

- **Resolve:**
  - The Inspector button at the top right of the UI toolbar opens the Inspector (pp. 734–744; [spec §8.15](spec.md#815-transform-and-crop-pp-766768)).
  - Position X and Y are timeline pixels from the middle, with Y up.
  - A picture narrower than the frame moves less than its number. On a 4:3 picture in a 16:9 timeline, Position X 200 moves it 150 pixels.
  - The width is the width that the scaling of the clip gives ([spec](spec.md#input-scaling-pp-292293)).
  - Crop is in pixels of the fitted picture. Zoom is a factor, and Rotation is in degrees.
- **vidcut:**
  - An Inspector tab is beside the Media and Titles tabs. It opens in the same place.
  - Position X and Y are frame pixels from the middle of the frame, with Y up. Thus Position X 200 moves every picture 200 frame pixels, whatever its shape.
  - Thus a new scaling of a clip keeps the place of its picture. In Resolve, the picture moves.
  - Zoom is in percent, as the viewer shows it while the user zooms.
  - Anchor X and Y are pixels of the mapped picture from its middle, with Y up. At 100% zoom, these are frame pixels.
  - The zoom of a Dynamic Zoom box is in percent. The X and Y of a box are where the middle of the picture is at the start or at the end.
  - They are in pixels of the mapped picture from the middle of the clip, with Y up. A new zoom of a box keeps the middle of the picture in place.
- **Why:**
  - The Titles tab of the Cut page is in the place of the Media Pool. The Inspector goes beside it. Thus the viewer and the timeline keep their size.
  - A number in frame pixels does the same thing to every picture. The viewer shows the zoom in percent. Thus one value does not show in two forms in vidcut.

## X18. Pitch and yaw are Inspector values

- **Resolve:**
  - The onscreen Transform controls set Zoom, Position, Rotation, Pitch, Yaw and Flip (p. 767).
  - The Inspector sets Pitch and Yaw too (p. 735).
- **vidcut:**
  - The Transform handles in the viewer do not set Pitch or Yaw.
- **Why:**
  - Pitch and yaw are numbers, as Rotation is.
  - The Inspector is where those numbers are set.
  - The viewer handles move, zoom, turn and crop the picture.

## X19. Upscale is On or Off, with AI by default

- **Resolve:**
  - Super Scale is in the Studio version only (p. 288).
  - It is a setting of a media clip, in Clip Attributes and in the Inspector of the Media page (pp. 288, 432–433).
  - It has 2x, 3x and 4x, each also as Enhanced. Sharpness and Noise Reduction are Low, Medium or High. Enhanced makes them variable (p. 433).
  - The project setting also has Auto (p. 149).
  - It makes a clip of a higher resolution, which often stops playback in real time (p. 288). The usual sizing then fits that clip.
- **vidcut:**
  - Upscale is Off or On. On enlarges a picture as much as it needs, up to 4 times its source. There is no Enhanced mode.
  - Its Upscaler is Project Settings, the default, AI Slow, AI Fast or Classic.
  - Project Settings has the upscaler of the project: Auto, the default, AI Slow, AI Fast or Classic. Auto is AI on macOS, and AI Fast elsewhere.
  - On macOS, the upscalers are AI and Classic. AI is the super-resolution scaler of macOS.
- **Why:**
  - vidcut makes a picture only as large as the output shows it. A factor would only limit placed pictures. Thus On is sufficient.
  - A video model adds detail that a classic upscaler cannot. Thus Auto gives AI.
  - macOS has its own AI scaler. Thus an export on macOS has other pixels than an export on other systems.

## X20. Deleting words on the timeline cuts at quiet points

- **Resolve:**
  - Delete of words in Timeline Transcription mode cuts exactly at the word times of the transcript.
  - The manual does not say this (pp. 1006–1008). Resolve 21.1.1 was tested.
- **vidcut:**
  - Delete, Remove Silent Portions and Delete Filler Words on the timeline move each edge to the quietest frame nearby (D63).
- **Why:**
  - The word times of a speech model are approximate.
  - A cut at those times can take the start of the next word, or leave part of a breath or a click.
  - A cut at a quiet point sounds cleaner.

## X21. Slow motion uses AI by default

- **Resolve:**
  - Retime Process is in the Retime and Scaling group of the Inspector (p. 739).
  - Its values are Project Settings, Nearest, Frame Blend and Optical Flow. The default is Project Settings.
  - The project's Retime Process is in Master Settings, Frame Interpolation (p. 145). In a new project of Resolve Studio 21.1, it is Nearest.
  - Optical Flow has a Motion Estimation mode (p. 740): Standard Faster or Better, Enhanced Faster or Better, and Speed Warp Faster or Better.
  - Speed Warp uses the DaVinci Neural Engine. AI Speed Warp Metal is a version of it for macOS only.
- **vidcut:**
  - Retime Process is Project Settings, the default, Nearest, Frame Blend, AI Fast or AI Slow.
  - The Retime Process of the project is Auto, the default, Nearest, Frame Blend, AI Fast or AI Slow.
    - Auto gives slow motion the AI of the computer that plays or exports: AI on macOS, AI Fast elsewhere.
    - Auto gives Nearest to the rest: clips at 100% or faster, and frozen and backward clips.
  - On macOS, the processes are Nearest, Frame Blend and AI. AI is the frame rate conversion of macOS.
- **Why:**
  - The AI of vidcut needs no paid version. Thus slow motion gets it by default.
  - Other speeds and frame rates gain less from made frames. Thus Auto keeps the speed of Nearest for them.

## X22. In an HDR timeline, AI Slow works before the conversion

- **Resolve:** the input goes into the timeline color space before Super Scale and before Speed Effects (§13, p. 3407).
- **vidcut:**
  - In an HDR timeline, AI Slow takes SDR media as SDR. The draw then converts its result into the timeline color space.
  - AI Slow refuses HDR media in an HDR timeline ([X19](#x19-upscale-is-on-or-off-with-ai-by-default)).
- **Why:**
  - In the test of `hdr_models` ([tools](tools.md)), AI Slow in PQ and HLG came far from the real frames, also with its color fix.
  - SDR goes into an HDR timeline with no clipping. Thus the conversion after the stage keeps all of the picture.

## X23. A new Smooth Cut is 0.2 seconds long

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

## X24. Some effects use a known method and come close to Resolve

- **Resolve:**
  - The rules of these effects are not public, or the renders do not show them.
  - The measurements are in Resolve Studio 21.1.2. The noise of a Glow is also measured in 21.2.
  - Gaussian Blur: Border Type sets what the blur takes from beyond the edges of the frame (p. 3536).
  - Gaussian Blur: the sigma follows the width of the timeline ([spec §8.16](spec.md#816-effects-pp-765-3526-3536)). Timelines of 1920×1080, 1920×1440 and 3840×2160 fit this rule within 0.1%.
  - Glow: Atmosphere multiplies the glow by a noise (p. 3590).
  - Film Grain: the renders show the spectrum and the statistics of the noise (p. 3559).
  - Dialogue Leveler: it finds dialogue with its own analysis (pp. 774–775).
  - Noise Reduction: it is a Fairlight FX with its own analysis (pp. 4041–4042). Its scripting API cannot set its values.
- **vidcut:** each effect uses a known method. The result is close to the renders, but it is not the same.
  - Gaussian Blur, Black border: black and clear pixels beyond the edge. Next to a white edge at Strength 1, Resolve gives 0.06 at the edge. One blur pass gives 0.27.
  - Gaussian Blur, Replicate border: the edge pixels, repeated. Resolve gives 0.64 at the edge. One blur pass gives 0.75.
  - Gaussian Blur on a portrait timeline: the width rule. On a 1080×1920 timeline at Strength 0.4, the sigma is 9.72 pixels. Resolve's sigma is 9.46, 3% less.
  - Glow up to a Spread of 0.1: the plain Gaussian curve. Resolve's blur has steps of 8 pixels. The largest difference is 0.0099, and the rms difference is about 0.0002.
  - Glow, Atmosphere: vidcut's own noise, with the statistics of the renders. The pattern is not the same.
  - Film Grain: vidcut's own noise, with the spectrum and the statistics of the renders. The pattern is not the same.
  - Dialogue Leveler: a part 10 dB above the lowest recent level is dialogue. With Allow Wider Dynamics, each line of the spoken test clip is within 1.3 dB of Resolve. With the other presets, a line is up to 5.1 dB away.
  - Noise Reduction: a spectral gate. On a spoken clip with noise at −60 dBFS, the noise between the lines loses 12.5 dB. The lines lose 0.0 to 0.7 dB. Resolve's default lowers the noise by 0.4 dB and quiet lines by up to 6.0 dB.
- **Why:**
  - A known method gives a result close to the measurements.
  - A full match needs the rules of Resolve, which are not public.

## X25. The render cache is made in the background

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
  - The codecs are Uncompressed 16-bit Float, the ProRes and DNxHR grades, HEVC and AV1, and Automatic. There is no Uncompressed 10-bit format.
  - Smart finds the media that are slow to decode by the decoder and by the proxy rules, not by a list of formats:
    - with GPU decoding on, media of a codec with long groups of pictures that the GPU does not decode
    - media that the proxy rules pick (spec D41), while the original plays
  - Smart also renders AI Upscale, and each Retime Process other than Nearest.
  - Smart also renders the parts where frames drop steadily during play ([D98](spec.md#12-decisions-on-gaps-and-contradictions)).
- **Why:**
  - The writes of playback would take GPU time from the frames on the screen.
  - The codec, the kinds of User mode and the time depend on the computer, not on the project. Its encoders, its decoders and its disk set them.
  - vidcut decodes H.264 and H.265 on the GPU. With the list of Resolve, Smart renders most clips of a camera or a phone.

## X26. A clip with an alpha channel is cached with the tracks mixed

- **Resolve:**
  - Uncompressed 10-bit, Uncompressed 16-bit Float, ProRes 4444, ProRes 4444 XQ and DNxHR 444 keep the alpha channel of a cached clip (p. 201).
  - Thus Resolve can cache a clip with an alpha channel at the clip level in these formats.
- **vidcut:**
  - Only Uncompressed 16-bit Float, ProRes 4444 and ProRes 4444 XQ keep the alpha channel. Media with an alpha channel is at the clip level only with these codecs ([spec D98](spec.md#12-decisions-on-gaps-and-contradictions)).
  - DNxHR 444 keeps no alpha channel. vidcut has no Uncompressed 10-bit codec ([X25](#x25-the-render-cache-is-made-in-the-background)).
  - At the timeline level, the cache holds the frame after the tracks mix. The frame looks the same.
  - The name of such a frame comes from all its layers, not from its time (`Cache::keys`). Thus its cache stays while its layers stay the same.
  - A change on another track at the time of the frame renders it again. A move of the clip renders it again where other frames are under the clip.
- **Why:**
  - The DNxHR encoder of FFmpeg has no pixel format with alpha.
  - The timeline level gives the same picture with one simple path. The cost is only more renders after edits.

## X27. With FFmpeg 8, vidcut refuses an animated WebP

- **Resolve:**
  - Resolve exports animated WebP files (p. 4116). It also imports them.
- **vidcut:**
  - With FFmpeg 8, vidcut refuses an animated WebP on import. The message says that the file needs FFmpeg 9.0.
- **Why:**
  - FFmpeg 8 reads the file as a still of 2×2 pixels, and its picture never renders ([gotchas](gotchas.md#ffmpeg-reading-and-decoding)).
  - A refusal gives the reason. A still that shows nothing does not.

## X28. One key has one action

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

## X29. DNxHR 444 exports without alpha

- **Resolve:**
  - Export Alpha works with DNxHR 444 (p. 4093).
- **vidcut:**
  - `video.alpha` refuses DNxHR 444. The Export page does not show Export Alpha for it.
- **Why:**
  - The FFmpeg encoder `dnxhd` cannot write alpha.

## X30. Every clip pans as on a stereo track

- **Resolve:**
  - The law of the Pan is different on a stereo track and on a mono track (measured in Resolve Studio 21.2).
  - The [spec](spec.md#9-audio-pp-642-649650-743744-771-773-778779) gives both laws.
  - The scripting API takes a Pan outside −100 to 100, and sets the nearest limit.
- **vidcut:**
  - Each clip uses the law of a stereo track, also on a track of another type.
  - A mix of one channel has no sides. The Pan does not change it.
  - A mix of three channels or more applies the Pan to its first two channels.
  - `clip.set` refuses a Pan outside −100 to 100.
- **Why:**
  - Resolve puts a mono clip on a stereo track by default. Thus the law of a stereo track is what most clips get.
  - A value outside the range is an error in the script. A refusal shows the error.

## X31. A compound clip refuses a clip on a locked track

- **Resolve:** ([spec §8.18](spec.md#818-compound-clips-pp-10521054-1126))
  - A selected item on a locked track stays out, and the other items go in.
- **vidcut:**
  - A clip on a locked track refuses the request.
- **Why:**
  - A locked track must not change. A selection that goes in only partly is a surprise. A refusal shows the lock.

## X32. ProRes 4444 and DNxHR 444 render cache files hold only 0..1

- **Resolve:**
  - The render cache formats ProRes 4444, ProRes 4444 XQ and DNxHR 444 keep the values below 0 and above 1 (pp. 201, 203).
  - Measured in Resolve Studio 21.2, with gray ramps from -0.1 to 2.0:
    - A cache file holds each value v as E = PQ(0.01 × |v|^2.4). Thus v goes through gamma 2.4, with 1.0 at 100 nits, and then through the SMPTE ST 2084 curve.
    - In DNxHR 444, one unit of E is 2628 codes of 12 bits for a v above 0. For a v below 0, it is 876 codes.
    - The steps of ProRes 4444 and ProRes 4444 XQ are half of the steps of DNxHR 444.
    - In DNxHR 444, one code is a step of 0.0016 near 1.0 and 0.0029 near 2.0.
- **vidcut:**
  - These three cache formats cut the values to 0..1.
  - In 0..1, they keep the full 10-bit precision: 876 codes, with a step of 0.0011.
  - The cache format Uncompressed 16-bit Float keeps all values.
  - Exports do not read the render cache. Thus an export keeps all values.
- **Why:**
  - The FFmpeg encoders prores_ks, prores_aw and dnxhd take only 10-bit input.
  - At 10 bits, the curve of Resolve gives 0..1 only 334 codes, in place of 876. One code near 1.0 is then a step of 0.0063.
  - When these FFmpeg encoders take 12-bit input, the cache keeps the values outside 0..1 with the curve of Resolve ([todo](todo/float-cache-and-ai.md)).
