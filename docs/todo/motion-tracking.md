# Motion tracking

**When:** now, one pull request for each step below.

A motion path is the path of an area of the picture through the frames of a clip. A clip stores motion paths, and the area of an effect follows a path ([model](../design/model.md#motion-paths)). `motion.find` makes a path for each thing of one kind in a clip ([analysis](../design/analysis.md#subjects)). `motion.track` makes a path for a drawn area.

In this file, a path is never a track of the timeline.

**Why:**

- A blur over a face or a number plate must follow it.
- A title or a picture that points at a thing must follow it.
- Without a motion path, the user sets a keyframe by hand at each move.

**What the manual says:**

- A search of the Cut page chapters (pp. 635–791) finds the Tracker palette only under Stabilization (pp. 738, 770).
- The Tracker palette of the Color page has three modes: Window, Stabilizer and FX (p. 3308).
  - Window mode moves a Power Window with a feature of the frame.
  - Stabilizer mode uses the same technology to steady the whole frame.
  - FX mode moves a Resolve FX or an OFX plugin that has position controls. It uses a point tracker.
- A track starts at the playhead. It runs forward to the last frame of the clip, or backward to the first (p. 3308).
  - One command tracks one frame (p. 3309).
- A track holds Pan, Tilt, Zoom, Rotate and Perspective 3D. The user chooses them before the track (pp. 3312–3313).
- The tracking data is apart from the values of the window, and those values can have keyframes (p. 3309).
  - A change of the window after the track offsets it from the tracked path.
  - A tracked effect takes an offset in the same way (p. 3495).
- A new track of a window replaces its tracking data (p. 3309).
- On a frame with no tracking data, the window stays where the first or the last tracked frame put it (p. 3310).
- Clear Selected Track Data removes a part of the data. Resolve then goes in a line across that part (p. 3315).
- Copy Track Data and Paste Track Data move a track between an effect and a window (p. 3311).
- On the Color page, a Resolve FX with position parameters takes a track (p. 3494).
  - The manual names Dent, Lens Flare, Light Rays, Mirrors, Radial Blur, Ripples, Vortex and Zoom Blur.
- IntelliTrack is a second point tracker of FX mode. Only Resolve Studio has it (p. 3496).
- The scripting API of Resolve Studio 21.2 has no call that makes or reads a track.

**In the code:**

- A clip holds motion paths, and `motion.add`, `motion.set`, `motion.remove` and `motion.list` write and read them ([deviation X165](../deviations.md#x165-a-clip-has-motion-paths)).
- Transform, Crop and the values of the effects have keyframes ([keyframes.md](keyframes.md)).
- A Glow stores Follow FX Tracker (`atmosphere_tracked`). The value does not change the picture ([spec §8.16](../spec.md#816-effects-pp-765-3526-3536)).
- An effect has an area: an ellipse or a box of the source picture, or of the area of a motion path ([deviation X166](../deviations.md#x166-an-effect-has-an-area)).
  - The center, the size, the rotation and the softness of the area have keyframes ([model](../design/model.md#pictures)).
- `motion.find` finds the faces, the text, the people and the vehicles of a clip with detector models ([deviation X167](../deviations.md#x167-vidcut-finds-the-faces-and-the-text-of-a-clip)).
- `motion.track` follows the thing under the area of an effect ([deviation X168](../deviations.md#x168-the-area-of-an-effect-follows-a-thing)).
- `motion.follow` makes a title follow a path of another clip ([deviation X169](../deviations.md#x169-a-title-follows-a-motion-path)).
- No request analyzes the motion of the camera. [Stabilization](stabilization.md) needs such an analysis.
- A [subject mask](subject-masks.md) is a matte that a motion path can move.

**How:**

- **A value follows a path.** Its own number and its keyframes are then an offset from the path.
  - Why: Resolve keeps the tracking data apart from the values. A new path then keeps the keyframes that the user set by hand.
- **vidcut finds the motion in two ways:**
  - A detector model finds each subject of one kind on each frame, with no click. vidcut joins the boxes of one subject into a path.
  - A tracker follows a box that the user draws, from the playhead forward or backward.
- Each of these is more than the Cut page has. The pull request that builds a part records its deviation.

**The models** (read on 2026-11-10 from the pages of their makers):

| Subject | Model | File | License |
|---|---|---|---|
| A face | YuNet, from OpenCV Zoo | 233 kB | MIT |
| A number plate | LPD-YuNet, from OpenCV Zoo | 4.15 MB | Apache 2.0 |
| A person and a vehicle. The model also has an animal and the other COCO classes. | YOLOX-S, from OpenCV Zoo | 35.9 MB | Apache 2.0 |
| A drawn box | VitTrack, from OpenCV Zoo | 715 kB | Apache 2.0 |
| A subject with its matte | EdgeTAM, or SAM 2.1 tiny | not read | Apache 2.0 |

- LPD-YuNet learned from Chinese number plates. Its maker says that it can miss other plates.
- YOLOX has smaller sizes at its maker: Nano with 0.91 M parameters, and Tiny with 5.06 M.
- EgoBlur finds faces and number plates under Apache 2.0. Each of its two models is about 400 MB.
- These do not fit:
  - CoTracker3: its license is CC BY-NC 4.0.
  - The number plate detectors of open-image-models: the page gives no license for the model files.
  - D-FINE: the page gives no license for the COCO model files, and it gives no ONNX file.
- Not checked: how well each model holds on real clips.

**To do:**

- **Area keyframes, what is left:**
  - The Inspector has no Keyframe button for the area of an effect. `effects.key` with `area` is the request for it.
  - The points of a polygon take no keyframes. Resolve animates a polygon window (manual p. 3273).
  - `motion.track` refuses an area with keyframes on its center, its size or its rotation. In Resolve, the keyframed transform of a window is an offset from the track (p. 3309).
- **The edges of a found path:** the model finds a face only when most of it shows. Thus a face that comes into the picture can show for a few frames with no blur.
  - Try how many frames a found path must start early and end late.
- **Faces, what is left:**
  - Check `motion.find` on a file with a rotation in its metadata. `subjects::find` turns the picture upright before the model sees it.
  - Measure YuNet on the GPU against the CPU, and build it there if it is clearly faster ([analysis](../design/analysis.md#subjects)).
    - Measured on 2026-11-10, on 16 threads, with the file decoded on the GPU:
    - A 720p file at 24 fps gives 253 pictures in a second. The model takes 61 % of that time.
    - A 2160p file at 59.94 fps in a project of 24 fps gives 57 pictures in a second. The model takes 21 % of that time.
  - The maker of YuNet gives face sizes from about 10 to 300 pixels. The model sees a picture with a longer side of 640 pixels.
    - Check a close shot where a face is larger than 300 of those pixels, and a wide shot with faces under 10.
    - Decide whether the model also sees the picture at another size.
- **More subjects:** number plates, animals and screens. The things that people blur most must be one choice each.
  - The buttons in the app for people and vehicles. Only `motion.find` of the CLI, IPC and MCP finds them.
    - The menus and the Blur row of the app list the subjects of `SUBJECTS` in `crates/vidcut/src/gui/inspector.rs`.
  - A search on 2026-11-10 found no number plate model that has a clear license, a small size and plates of many countries.
    - The models with a clear license know the plates of one country, or have about 40 MB and attention layers.
  - Check on a picture with a number plate whether the text subject finds its characters. No test clip has a number plate.
  - The YOLOX-S model of the people and the vehicles also has classes for animals and screens.
- **People and vehicles, what is left:**
  - Check the vehicle subject on a clip with vehicles. No test clip shows one.
  - The model can give one person two boxes, for example with a thing that the person holds. Then one person can get two paths.
  - The model runs on a square of 640 pixels, as the code of its maker does. Try a model input with the shape of the picture.
    - Measured on 2026-11-11, on 16 threads, with the file decoded on the GPU:
    - A 720p file at 24 fps gives from 21 to 24 pictures in a second. The model takes 94 % of that time.
    - A 2160p file at 59.94 fps in a project of 24 fps gives from 16 to 17 pictures in a second. The model takes 70 % of that time.
- **The size of the picture of a find:** `motion.find` takes `size`. The app has no control for it.
- **Text, the open parts:**
  - Very small text: try a longer side above 960 pixels for a large source, and measure the time.
  - Text that comes into the picture has no blur on its first frames, as a face has not.
  - The detector takes most of the time of a text find. Try the model on the GPU.
- **The track of an area, what is left:**
  - Measure `tracking::LOST` on a clip where the thing leaves the picture, or goes behind another thing. No test clip has one.
  - A track follows no turn and no change of perspective.
- **The Motion section in the app:** click Blur in a window, and take a find that asks for the download of its model. Find ran from its button.
- **A clip follows a path** of another clip: a picture in a picture, or a sticker.
- **A title that follows a path, what is left:**
  - Try Follow in the app: the menu, a drag of a title that follows a path, and its zoom and turn handles.
  - The size and the angle of the thing do not move the title.
  - A title inside a compound clip follows no path.
  - An OpenTimelineIO file does not keep what a title follows.
  - Measure on the Fusion page where a connected node stays after the last frame of a track.
  - Measure on the Fusion page what a connection does to a Center with keyframes. Until then, `motion.follow` refuses a title with keyframes on its position.
  - Measure on the Fusion page where a node goes when its connection to a tracker is removed. Until then, a title that stops following keeps its numbers.
- **A subject with its matte:** with [subject masks](subject-masks.md).
- **Resolve measurement:**
  - Measure the lines of the manual above in Resolve. The manual and the application can differ.
  - Measure whether a track of the Color page plays on the Cut page, and what Follow FX Tracker does with it.
  - Measure what an OpenTimelineIO file keeps of a track.
- The CLI, IPC and MCP perform the same action as the GUI, in each step.
