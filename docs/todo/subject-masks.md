# Subject masks

**When:** now. The owner approved the plan on 2026-11-26.

A subject mask is the Magic Mask palette of the Color page. Clicks on a person or a thing make the key of a corrector. The key follows the subject through the clip.

**Why:**

- vidcut has no matte of a person, and no matte of a thing that the user points at.
- Resolve Studio has this palette on the Color page and a node with the same tool on the Fusion page. Thus it is no deviation.

## What Resolve does

The pages are in the Resolve 21.1 manual. The installed Resolve is Studio 21.2.

Pages 3288–3304 tell of version 1. A rule from these pages has the mark "version 1 text". The R runs check each such rule on version 2.

- **Versions:**
  - Version 2 replaces version 1 (p. 3288). Only Resolve Studio has version 2 (p. 3281).
  - Version 1 stays for older projects, under Legacy Object Mask in the options menu (pp. 3287–3288).
  - vidcut builds version 2 only ([AGENTS.md](../../AGENTS.md), rule 13).
  - Version 2 has no Person mode and no Object mode. One tool masks people and things (p. 3281).
- **Clicks:**
  - A positive click (blue) adds the person or thing under it. A negative click (red) takes a part away (p. 3282).
  - A click on a part of a person gives that part, for example the face, the hair, the arms or the shoes (p. 3281).
  - A drag moves a click. The pointer selects a click, and Delete removes it (p. 3282).
  - The Click list selects, turns on, turns off and deletes each click (p. 3282).
  - The timeline of the Click list shows the tracked frames and a mark at the reference frame (p. 3286).
- **Tracking:**
  - The track buttons track all clicks at once. Each click follows the picture around it (p. 3283).
  - After a track of the clicks, each frame of the shot gets its mask. A change of a click changes the mask at once (p. 3283).
  - The buttons are Go To Reference Frame, the first and the last tracked frame, and one frame back or forward (p. 3285).
  - Track Reverse, Track Forward, Track Forward and Reverse, and Stop Tracking track the rest of the clip (p. 3285).
  - The tracking bar shows the tracked frames in blue (p. 3283).
- **Paint:**
  - An add brush, a subtract brush and a brush size touch up the mask (p. 3286).
  - A paint stroke acts on its own frame only. It does not track (p. 3283).
- **Invert Mask** inverts the mask. **Mask Overlay** tints the masked part translucent red (p. 3286).
- **Quality:**
  - Faster gives a looser mask quickly, for garbage mattes (p. 3286).
  - Better gives more detail and takes more work (p. 3286).
- **Smart Refine** grows or shrinks the mask along the edges of the subject, in Better. It adds no softness (p. 3286).
- **Matte Finesse:** Mode, Shape, Radius, Smoothing, Blur Radius, Clean Black, Clean White, Iterations, Denoise, In/Out Ratio, Black Clip, White Clip and Post Filter (pp. 3286–3287).
- **Consistency** smooths the edges of the mask over time. It is 0 at first. It needs clicks of a few tracked frames (p. 3287).
- **Render in Place Magic Mask** writes the mask frames to a folder that the user selects. It adds an external matte node with them (pp. 3287–3288).
- **The key of the node:**
  - In one node, the Magic Mask and a qualifier key give their intersection. The Magic Mask and a window also do (p. 3289, version 1 text).
  - The manual shows no Magic Mask row in the Key palette (p. 3436).
- **Use Fast Tracking** in the options menu tracks faster and less exactly (p. 3303, version 1 text).
- **Alpha Output:** a key on the Alpha Output of the Node Editor makes the clip transparent (pp. 3390, 3427, 3446–3447).
- **Fusion:** the Magic Mask node has the same tool, a disk cache, a Reference Time and a Matte tab (pp. 2534–2538).
- **Limits:** the mask has hard edges. It does not make the soft edges of hair as a keyer does (pp. 3292, 3297, version 1 text).

## In the code

- A corrector has windows, a qualifier and a key input. `key_steps` folds them on the GPU in 32-bit float (`crates/vidcut-core/src/grade.rs`).
- The Color page has no palette that tracks. It has no Tracker palette ([motion tracking](motion-tracking.md)).
- `motion.track` follows a box with VitTrack, in tract on the CPU. `motion.find` finds people with YOLOX-S.
- A corrector has a Magic Mask with clicks, a track of the clicks, Invert Mask, Quality and the key ([spec](../spec.md#829-magic-mask-pp-32813289-3292-32963297), [design](../design/analysis.md#subject-masks)).
  - SAM 2.1 makes the maps in vidcut's own GPU kernels. The requests are `grade.mask.click`, `grade.mask.set`, `grade.mask.track` and `grade.mask.clear`.
  - Quality selects the model: tiny for Faster, base+ for Better. The preference `color.magic_mask_large` selects large for Better.
  - A track carries the clicks of one frame forward and back, with the video memory of SAM 2.1.
  - With `gpu.acceleration` off, the same kernels run on the software adapter of wgpu. A frame of a track takes about 50 s there.
  - The mask store keeps the maps in memory and in the disk cache (`disk_cache::Kind::Masks`). An export makes a missing map again.
  - The viewer shows no mask on a frame whose map is missing.
  - `masks::RULES` and `MASK_CUT` hold the rules that the R runs measure. Until then they are vidcut's choice.
- The catalog downloads each model on first use, after an ask, and checks its SHA-256 (`catalog.rs`, `models.rs`).
- RetinaFace, NanoVSR, FlashVSR and SAM 2.1 run in vidcut's own GPU kernels. The other ONNX models run in tract on the CPU.
- The Node Editor has no Alpha Output. A clip with its own alpha composites over the tracks below.

## The models

All three are SAM 2.1 of Meta. The code and the checkpoints are Apache 2.0. The numbers are from the SAM 2 README.

| Quality | Model | Parameters | File | SA-V test J&F | MOSE val J&F | LVOS v2 J&F | Pictures a second on an A100 |
|---|---|---|---|---|---|---|---|
| Faster, the usual model | SAM 2.1 tiny | 38.9 M | 156 MB | 76.5 | 71.8 | 77.3 | 91.2 |
| Better | SAM 2.1 base+ | 80.8 M | 324 MB | 78.2 | 73.7 | 78.2 | 64.1 |
| Better, a choice in the preferences | SAM 2.1 large | 224.4 M | 898 MB | 79.5 | 74.6 | 80.6 | 39.5 |

- J&F is the standard score of a video mask: the overlap with the true mask and the fit of its edge. It goes from 0 to 100, and higher is better.
- The checkpoints download from Meta, at `https://dl.fbaipublicfiles.com/segment_anything_2/092824/`.
  - The files are `sam2.1_hiera_tiny.pt`, `sam2.1_hiera_base_plus.pt` and `sam2.1_hiera_large.pt`.
- License pages: [the SAM 2 README and LICENSE](https://github.com/facebookresearch/sam2), [the model card](https://huggingface.co/facebook/sam2.1-hiera-tiny) (tag apache-2.0).
- The training data:
  - The README does not name the training data of the checkpoints.
  - SA-V is CC BY 4.0. SA-1B has a research-only license. A secondary source also names MOSE (CC BY-NC-SA 4.0).
  - The checkpoints themselves are Apache 2.0. vidcut does not distribute them. Each one downloads from Meta on first use.
  - The owner accepts this caveat, as for UNISAL.
- The video logic is outside the network: the memory of past frames, the frames with prompts, and their time codes. vidcut writes it.
- These later choices change only that logic. They keep the SAM 2.1 checkpoints:
  - DAM4SAM (SAM2.1++) keeps the subject apart from look-alikes. Apache 2.0. LaSOT 70.0 to 75.1, about 20 % slower ([page](https://github.com/jovanavidenovic/DAM4SAM)).
  - SAMURAI adds a motion filter and a stricter choice of the frames in memory. Apache 2.0. LaSOT AUC 68.54 to 74.23 on large ([page](https://github.com/yangchris11/samurai)).
- Other makers publish ONNX files. Meta publishes none.
  - [jax-image-tools/sam21-tiny-video-onnx](https://huggingface.co/jax-image-tools/sam21-tiny-video-onnx): tiny only, four graphs, 158 MB, Apache 2.0.
  - [ailia-models](https://github.com/axinc-ai/ailia-models/tree/master/image_segmentation/segment-anything-2): all sizes, seven graphs. The README gives no download link for the files.
  - tract 0.23.10 has each op that these graphs use. It has no `Loop`. A load was not tried.
- These do not fit:
  - SAM 3 and SAM 3.1: the SAM License limits uses and forbids reverse engineering.
  - SAM2Long: the code is CC BY-NC 4.0. DEVA: CC BY-NC-SA 4.0. Robust Video Matting: GPL-3.0.
  - Human parsing models for named parts: each has a non-commercial license, or no license for its weights.
  - For one click, SAM 2.1 gives three masks, for example the whole person, a part and a smaller part (the SAM paper of Meta).
  - vidcut keeps the mask with the best predicted overlap. A click on a face can thus give the face, or the whole person.
  - No run measures this yet. M1 and R6 have the cases.
- Smaller models fit but score lower: EdgeTAM (SA-V test 71.7), EfficientTAM S (74.5), Cutie (MOSE 68.6 to 69.9).

## How vidcut builds it

- The Magic Mask is a part of a corrector, as its windows and its qualifier are.
  - Its key joins the window key and the qualifier key as an intersection (p. 3289, version 1 text). Run R2 measures the formula.
- The project keeps the clicks and the settings:
  - Each click has its sign, its on or off state, its reference frame and its place on each tracked frame.
  - The settings are Quality, Invert, Smart Refine, Consistency, Matte Finesse and the paint strokes.
- The model reads the picture that comes into the corrector, as the qualifier does. Run R3 checks this.
  - The model's input is that picture, clamped to 0..1 and scaled to its square. The clamp is part of the model step, not of the picture path.
- For each frame and each subject, the model gives a map of 256 × 256 values.
  - A mask store keeps the maps on disk, in a new `disk_cache::Kind` under one byte limit of the preferences.
  - The maps in memory use `lru::ByBytes`.
  - vidcut can make the maps again from the clicks. A render with missing maps tracks those frames again first.
- The key stage scales the map to the frame on the GPU and cuts it at a level. The key is 32-bit float at each frame size.
  - Smart Refine moves the level. The mask then grows or shrinks along the edges of the subject, with no softness.
  - Run R4 compares this with Resolve.
- A positive click outside the masks starts a new subject. A click on a mask changes that subject.
  - The mask of the node is the union of its subjects (p. 3296, version 1 text). Run R6 checks it.
- A track gives the frames to the model one by one, from the reference frame forward or back.
  - The clicks are the prompts on their reference frames.
  - A drag of a click on a tracked frame gives that click a new reference frame (p. 3304, version 1 text).
  - Run R5 shows how the marks of the clicks move on the tracked frames.
- The model runs in vidcut's own GPU kernels, as FlashVSR does. `weights.rs` reads the checkpoints of Meta.
  - Why: the GPU comes first ([AGENTS.md](../../AGENTS.md), rule 9), and Meta's own link has only these checkpoints.
  - The kernels in `compute` have convolutions, layer norm and windowed attention.
  - SAM 2.1 also needs full attention with rotary positions.
- Each action of the palette is a request: `grade.mask.click`, `grade.mask.set`, `grade.mask.track` and `grade.mask.clear`.
  - The schema gives them to the CLI, IPC and MCP. Stop Tracking uses the cancel of the other analyses.
  - Undo covers each request, also a track.

## Steps in order of value

Each step is one pull request.

- **Palette:** the Magic Mask palette, the click tools, the Click list, Mask Overlay and Quality.
  - The switch of `color.magic_mask_large` in Preferences > Color.
  - The track buttons, Stop Tracking and the timeline of the Click list.
  - The viewer makes a missing map again, as the export does.
- **The measured rules:** set `masks::RULES` and `MASK_CUT` to the results of runs R3–R6.
  - R5 shows whether a track goes past a frame with clicks, and which map such a frame keeps.
  - R5 also shows whether the clicks of more than one frame make one track. SAM 2.1 can take the clicks of more frames into its memory.
- **A CPU path:** a frame of a track takes about 50 s on the software adapter. macOS has no software adapter.
  - A port of the network to the CPU kernels of vidcut would make the CPU path usable.
- **Speed:** a frame of a track at 1080p takes about 0.10 s with tiny, 0.13 s with base+ and 0.20 s with large.
  - These times are for one subject, on the GPU of the test computer, with the memory full.
  - The image encoder takes about 36 ms with tiny, 63 ms with base+ and 130 ms with large.
  - The memory attention and the mask decoder take about 65 ms with each model.
  - On that computer, a track of 100 frames also stops once.
  - The stop takes about 2 s with tiny, 3 s with base+ and 7 s with large.
  - Still to do: find the cause of this stop ([gotchas](../gotchas.md#gpu-drivers)).
- **A change before the node:** the name of a map is a digest of the input picture at the time of the click.
  - An export makes a missing map from the picture of now, under the old name (`editor::mask_remakes`).
  - Thus, after a change before the node and a lost map, the export can show another mask than the viewer showed.
  - A change of `color.magic_mask_large` has the same effect on a lost map of Better.
  - Run R3 shows what Resolve does. vidcut then makes the mask again, or keeps it, as Resolve does.
- **Alpha Output:** the Alpha Output of the Node Editor. The mask then makes the clip transparent over the tracks below.
- **Smart Refine and Consistency.**
- **Matte Finesse:** one set of controls for the qualifier and the Magic Mask ([Color page](color-page.md)).
- **Paint:** the add brush, the subtract brush and the brush size.
- **Memory choices:** DAM4SAM and SAMURAI, if they give a better J&F on DAVIS 2017.
- **Render in Place:** the external matte node and the written mask frames.
- **A Magic Mask node on the Effects page.**

## Measurements

The owner approved the downloads of vidcut's runs.

- The files go to `sam2` in the test data folder, never into git.
- DAVIS 2017 states no license. Its pages ask for a citation. The set is only for these runs.

These are vidcut's runs:

- **M1, port check:** the official SAM 2.1 code and vidcut on DAVIS 2017 val: 30 videos, 1,999 frames.
  - The official code runs in Python, in a uv environment outside the repository.
  - The same clicks go on the first frame of each video, for each of the three models.
  - The overlap of vidcut's mask with the official mask on each frame: the mean and the worst. Higher is better. The aim is 0.99.
  - J&F of both against the true masks of DAVIS.
  - The official J&F with one click on the first frame, one run of each model: tiny 68.0, base+ 68.7, large 76.7.
  - Done for each model: frame 0 of the 30 videos, 61 objects, one positive click on each.
    - On the same input, vidcut and the official network overlap on average 0.9999 (tiny), 0.9999 (base+) and 1.0000 (large).
    - The worst overlaps are 0.9993 (tiny), 0.9976 (base+) and 0.9997 (large).
  - Still to do on frame 0: two positive clicks, a positive and a negative click, and a click near the edge of the frame.
    - Also one click each on a face, on the hair and on the shoes of a person.
    - Why: one click does not test the choice of the output for more clicks, or the part that a click gives.
  - Still to do: one frame each of the five clips of the Resolve runs, with the same clicks for both.
    - Why: each DAVIS frame is 854 × 480. The scale to the input square changes with the frame size.
  - Done for tracks of each model on the same input: 12 frames of blackswan, and two objects on 12 frames of bike-packing.
    - The masks of vidcut and of the official video predictor overlap 0.9999 (tiny) and 1.0000 (base+ and large) on average.
  - Still to do: J&F of vidcut's tracks on all 30 videos, against the official 68.0.
- **M2, cost:** 10 s each of tos-720p-mov, pedestrian-area, stab-crowd-4k, stab-dashcam and stab-rally.
  - One click, a track forward, each model, GPU decoding on.
  - Seconds per minute of video, CPU time, GPU use and peak GPU memory.
  - A video of each mask over the picture, for the owner to look at.
- The time of each run is known after the first video. I stop and ask if the total passes 2 hours.

The Resolve runs are approved. They wait for a free slot on Resolve. They are in the project "vidcut measure", at 1920 × 1080 and 3840 × 2160.

Two kinds of rule need different runs:

- A simple fact needs one look. Examples: a menu item, a shortcut key, a range and a usual value.
- A behavior that depends on the picture needs several measurements, on clips that differ.
  - Examples: the subject that a click picks, the edge of the mask, tracking, and the effect of a change before the node.
  - Each such rule gets three sources at least. They differ in content, number of subjects, subject size, motion and resolution.
  - Each such rule also gets other values, the ends of each range, and inputs where a simple rule gives a wrong result.
  - One measurement records only what Resolve did for that input. It is not a rule.

The sources are five clips of the test data:

| Source | Size | Rate | Content |
|---|---|---|---|
| stab-crowd-4k | 3840 × 2160 | 30 fps | a handheld walk in a crowd: many people, some small and far |
| nasa-roman-presser-full | 3840 × 2160 | 59.94 fps | a still camera. One person close up at 600 s. Six people at a table at 610–630 s. Five people side by side at 695 s |
| stab-rally | 1920 × 1080 | 30 fps | fast motion: cars, people at 4–6 s, one car close up at 18–19.5 s |
| tos-720p-mov | 1280 × 534 | 24 fps | a film: one or two actors, close shots, cuts |
| jiku-gt090912-phone3 | 720 × 480 | 29.5 fps | a phone at an event: low resolution, many people |

The times in tos-720p-mov and jiku-gt090912-phone3 are chosen before the runs.

The runs:

- **R1, palette:** the controls of version 2 in Resolve 21.2, their ranges, their usual values and the options menu.
  - These are simple facts. One look at each is enough, on one clip.
  - A typed value past each end of a range shows the limit.
- **R2, the key:** float renders of one node with a Magic Mask, a circle window and an HSL qualifier.
  - Each key alone, each pair of keys, and all three. The manual names the pairs only (p. 3289, version 1 text).
  - Window softness at 0, at a middle value and at the largest value.
  - A qualifier with no soft range, and a qualifier with a wide soft range.
  - Why: a soft window and a soft qualifier together tell a product from a minimum. A hard mask alone cannot.
  - Invert off and on, for the Magic Mask, the window and the qualifier.
  - The Key palette: Key Input Gain and Key Output Gain, each at its usual value and at two other values.
  - Three sources: a person of crowd, a car of rally and an actor of tos-720p-mov. Both sizes.
- **R3, the input:** does a change of an earlier node change the mask, and does it clear the tracking?
  - Node 01 gets a small gain (1.05), a large gain (2.0) and saturation 0, one at a time.
  - Node 01 gets a value that is then set back. Why: the picture stays the same. Thus this tells a check of the picture from a check of the node.
  - A change after the Magic Mask node is the control. It must not change the mask.
  - Each change on the reference frame, and on tracked frames 1, 12 and 23 frames from it.
  - Four sources: crowd, nasa, rally and tos-720p-mov.
- **R4, the edge:** the values of the mask at its edge, for Faster and Better, and the effect of Smart Refine.
  - Smart Refine at its low end, at 0, at its high end, and at two values between.
  - Faster and Better with the same clicks.
  - One edge in each source: hair close up (nasa, tos-720p-mov), a car (rally), a small far person (crowd) and a person at low resolution (jiku).
  - Both sizes. They show whether the edge width follows the frame size.
  - The manual says that the mask has hard edges (pp. 3292, 3297, version 1 text). Values between 0 and 1 at the edge show a soft edge.
- **R5, the clicks:** the place of each click on tracked frames, and the mask on a frame with no tracking.
  - Tracked frames 1, 12 and 23 frames from the reference frame, forward and back.
  - Each kind of motion: a still camera (nasa), a handheld walk (crowd), fast motion (rally), a cut (tos-720p-mov) and low resolution (jiku).
  - Clicks in the middle of the frame, near each edge of the frame, and on a small subject.
  - A subject that leaves the frame, and a subject that goes behind another.
  - A drag of a click on a tracked frame and on the reference frame (p. 3304, version 1 text).
  - A frame before the first tracked frame, after a track forward only.
- **R6, many subjects:** the subject of each click, and the mask of the node.
  - Shots with one subject (nasa at 600 s), two (tos-720p-mov), five or six (nasa at 610–630 s and at 695 s), and many (crowd, jiku).
  - Two people apart, and two people that touch or overlap. Then a third subject.
  - A positive click inside a mask, outside all masks, and on the overlap of two masks.
  - A negative click inside one mask, inside the other mask, and outside all masks.
  - The union of the subjects (p. 3296, version 1 text), where two masks overlap.
  - One click each on a face, on the hair and on the shoes, on a person of three sources (p. 3281). vidcut gets the same clicks.
- **R7, keyframes and copies:** do the values take keyframes, and does a copy of the node keep the tracking?
  - A keyframe on each value of the palette. This is a simple fact.
  - Copies before a track and after a track.
  - Copies to the same clip, to another clip of the same file, to a clip of another file, and to the other frame size.
  - A copy of the node, a still of the Gallery in an album of these runs, and Apply Grade.
  - If a copy tracks again, its mask depends on the picture. Then two more sources check it.
- **R8, the Alpha Output:** the transparency of a clip over V1, with the mask on the Alpha Output.
  - Invert off and on.
  - A soft window in the same node. Key values between 0 and 1 show whether the alpha follows the key in a straight line.
  - Over flat gray and over a picture on V1.
  - Three sources: crowd, rally and tos-720p-mov. Both sizes.
- The runs need computer use on Resolve, because the clicks are in the viewer. They take about 6 hours, in more than one slot.
  - If a DRX still keeps the clicks, scripts make the variants from DRX files. Then the window block is shorter.
