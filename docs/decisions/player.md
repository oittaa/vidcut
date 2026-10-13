# The player engine

vidcut's own playback engine (`vidcut_av::player`), tested headless. The GUI's viewer plays the source and the timeline through one engine ([preview.md](preview.md)).

Tests: `crates/vidcut-av/tests/player.rs`, `player_sound.rs`, `player_decodes.rs`, `player_media.rs`, `player_gpu.rs` (`--features vidcut-av/gpu-tests`), `player_device.rs` (`--features vidcut-av/audio-tests`).

## 1. Threads

| Thread | Does | Never |
|---|---|---|
| **Caller** (the live session sends requests; the viewer thread presents) | sends requests, which return at once; `present` looks up a picture | decodes or waits |
| **Decode** (`vidcut-play-video`) | decodes the picture of the viewer's frame, then pictures ahead of it | touches the device |
| **Audio** (`vidcut-play-audio`) | mixes the plan's lanes into the ring; while paused, a step's frame | waits on the device |
| **Device callback** (the output's thread) | copies from the ring and records when it will be heard | locks, allocates or decodes |
| **Openers** (`vidcut-open`, the export's, one per reader) | open the next file ahead | |
| **Scrubber** (`vidcut-scrub`, only with a `Scrubber`) | mixes the frame asked for | |

vidcut-av starts these with `std::thread::Builder`, named `vidcut-…`. They keep normal priority: playback is interactive work (`threads::spawn`'s rule, gui.md §9).

## 2. Requests: one wanted state, merged and cancelled

- **One wanted state.** A mutex holds what is wanted: the plan, the mode (paused at a frame, or playing from a sample at a speed), lane gains, mute and audio scrubbing. A request changes it and wakes both threads, which read the newest state. Nothing queues: requests that arrive while a thread works merge into the last one.
- **Position.** Paused, it is the frame asked for; playing, it is the clock (§4).
  - `step` pauses where the viewer is and moves from there.
  - `play` while playing continues from the sample heard now, at the new speed.
  - `play` forward from the last frame starts over from the first; backward from the first frame, from the last.
- **Cancelling.** The decoders check the decode thread's `vidcut_av::cancel` flag. When the new frame's picture isn't kept, the decode in progress is cancelled if:
  - it was only decoding ahead,
  - the player is or was playing, or
  - it is a paused seek that has run for a second (`PATIENCE`).

  Otherwise the paused seek finishes and the newest request runs next. So a held key moves the picture at the speed frames decode, and the last request lands exactly (gui.md §4).
- **After a cancel** the reader forgets its position (its next piece seeks); the file stays open.

## 3. Pictures: decoded YUV, for a shader

- **Chosen by the export's reader** (`export::video::VideoFrames` in its `decoded` mode): the sampling rule (export.md §2), deinterlacing, stills and image sequences. So each picture is the frame export shows.
- **Kept as decoded.** A picture is the decoded frame, held by reference: its own size and colors, not rotated. The layouts are those a display takes: 8-bit 4:2:0 (planar, or NV12 from hardware decoders) and P010 for more bits.
  - Other layouts are converted to the nearest one at their own size (`Relayout`). 4:2:2, 4:4:4 and 10-bit planar become 4:2:0, keeping their matrix and range (ProRes 4:2:2 10-bit becomes P010). RGB images are coded as BT.709 video range.
  - Black is the plan's size, in 8-bit 4:2:0.
  - Nothing copies a picture's pixels after the decoder.
- **The display does the rest, on the GPU:**
  - fits the picture into the viewer as export does (`Picture::placement`);
  - rotates it by the display matrix (`Picture::rotation`);
  - converts YUV to BT.709 RGB using `Picture::colors` (matrix, range, HDR). That is `vidcut_av::color`'s reading of the frame: its tags, or for untagged pictures BT.709 when HD, BT.601 otherwise, BT.2020 when HDR.

  `Picture::to_rgba` does the whole path on the CPU with the export's own filters. It gives what `frame render` gives, and a shader must match it.
- **GPU decoding** (`Config::hardware`, `set_hardware`): streams the GPU decodes go to `Config::gpu`, or else the best decoder there is (`hw::GpuDecoder::best`); the rest decode on the CPU. Which decoders and streams: preview.md §3.
  - With `Config::gpu_pictures`, the GPU's frames (NV12 or P010) stay in video memory (`Memory::Cuda`, `Memory::Vaapi`, `Memory::D3d11`) for a display that shows them from there. Otherwise they are copied down to memory at their own size, with their properties.
  - Kept frames don't starve the decoder of surfaces (preview.md §3), so the store keeps as many as the budget holds.
  - Switching the decoder decodes the pictures again. `Picture::decoder` says what decoded a picture; `Status::decoder` is the one shown last.
- **GPU surfaces.** `Picture::memory` and `Picture::layout` read a hardware frame's format and frames context (CUDA, VAAPI, D3D11, Vulkan). `Picture::bytes` is what its layout takes, so the budget counts video memory as it counts memory. The engine never reads the pixels. `copied_down` (and `to_rgba`, through it) brings a picture to memory, for a display that can't show it from video memory and for tests.
- **Keyed by what they show:** the file, its origin, still or sequence, the source position and the rate (black by the plan's size). So pictures survive edits that move them (a ripple), and seeking to a kept picture shows it without waiting for the decode thread.
- **Budgets** (`Config::budget`; `set_budget` changes them while playing): one for pictures in memory, one for pictures in video memory, each counted where the pictures live. Over a budget, that kind's least recently kept or shown pictures go first. The prepared picture never goes.
  - `Budget::automatic` is a viewer's: the larger of 1 GB and half the computer's memory, and for video memory half the GPU's memory once the decode thread opens NVDEC. VA-API and Direct3D 11 don't report their memory, so there it is 1 GB.
  - `DEFAULT_BUDGET` (1 GB of each) is for tests and tools.
- **Forward**, the reader decodes on from frame to frame, and the next file opens ahead on the export's opener thread. Frames the clock has already passed are decoded but not kept. A reader more than a second behind seeks to where the clock will be once the seek is done (the recent seek time × speed) and continues from there.
- **Ahead:** a quarter of a second × speed while playing (at most a quarter of the budget). Paused, three frames, when the reader is already there.
- **Back:** a window from a keyframe up to the frame, all kept, at most half the budget (the same rule as the two-up renderer, preview.md §8). A window starts on a keyframe. It reaches back over another keyframe only if that whole GOP fits, because starting between two keyframes would decode the frames before the start for nothing.
- **Playing backward.** A frame with nothing kept decodes from its own keyframe, so it shows after one GOP of decoding, not two.
  - Two more windows wait behind the one shown, where they fit. Once only one is left, the one before it decodes.
  - The first window behind is the nearest, from its own keyframe, since the playhead may be close to it.
  - The second holds at least two seconds where keyframes are close. It decodes once it fits in the budget beside the frames still to be shown.
  - Before a window decodes, those frames still to be shown are marked as looked at, so when the budget is full, frames already shown go first.
- **Around a paused playhead** (whichever way it last moved), at least two seconds behind it are decoded while nothing else is wanted (at most half the budget), nearest first, so the first steps back find their pictures.
  - Each window runs from the keyframe of the latest missing frame up to it.
  - In video memory a window decodes all at once. In memory it decodes half a second at a time, because each picture kept there takes new memory, and a 10-bit one also a conversion. So a step right after a seek waits for half a second of frames, not the whole GOP.
  - A step back into the window being decoded waits for it rather than restarting it (`Busy::Window`). A step past what is kept decodes such a chunk itself.
- **A seek** decodes from the keyframe before its frame and keeps what it decodes on the way: all of it in video memory (where kept frames cost nothing more), the last half second in memory (for the reason above). So steps back right after a seek find their pictures; the rest behind comes while nothing else is wanted. Finding a window's keyframe takes one seek and one decoded frame, and the window decodes on from there (`VideoReader::from_keyframe`), however far into the GOP it starts.
- **Edits.** `set_plan` keeps the position, the open file and the kept pictures. The reader reads the new plan's pieces: a piece of the open file decodes on or seeks.
  - The sample rate and channels must not change, since the device was opened for them. A plan with others is refused before anything changes.
  - The same plan again changes nothing. Neither case cancels a decode.
  - While playing, the sound continues from the same time. At another frame rate, the picture shown stays until the new plan's picture is decoded.
- **The plan played next.** `prepare` names a frame of another plan, such as the timeline's while the viewer shows the source.
  - When paused with nothing else to decode, the decode thread decodes it on a reader of its own, made for that picture, so the playing reader keeps its position. A new request cancels it, as it cancels decoding ahead.
  - The store holds its picture, whatever the budget, until another is prepared (`None` releases it). `Status::prepared` says whether it is decoded; `on_picture` is called when it lands.
  - `set_plan_at(plan, frame)` then plays that plan from that frame in one change, decoding nothing for frames in between. If the picture is decoded, the next `present` shows it; if not, the picture shown before stays until the new one lands.
  - A new plan forgets which way the viewer last moved: steps back in one plan say nothing about the other, so no window behind is decoded there.
  - Both plans share one budget. Playing one can evict the other's windows behind, so the other's first step back after a switch may decode a window again. The prepared picture stays, so the switch itself shows at once.

## 4. Sound, and the clock

- **The mix** is the export's (`export::audio::AudioMix`): every lane, clip gains and faders as the plan has them, de-click fades at cuts. The audio thread mixes 10 ms at a time into a ring of one second, keeping a tenth of a second queued. A chunk is never dropped: the thread waits for room.
- **Live faders.** `set_lane_gain` puts a gain on top of a lane's own (the Mixer's fader while it is dragged). It is heard within a tenth of a second plus the device's latency. A new plan drops these gains, since by then the saved level is in the plan.
- **Mute** writes silence; the clock keeps running.
- **The ring** is lock-free, with one writer and one reader. Positions count sample frames since the player started and never wrap. Seeks, pauses, new plans and new speeds drop what was queued.
- **Markers** map ring samples to the timeline: from ring position *c*, timeline sample *s*, moving *v* timeline samples per ring frame (negative backward), for play request *e*. The end of the plan (or its start, backward) gets a marker that holds still.
- **The device's callback** copies from the ring (silence when it is empty). Lock-free, it records the ring position of its first sample, how many ring frames it took, and when the first is heard: now plus the output's latency.
- **The clock** at time *t*: take the newest buffer heard by *t*, go as far into it as *t* reaches (no further than the frames it held), and map that ring position through the markers to a timeline sample. The viewer shows the frame holding that sample: frame *j* spans samples `[frame_to_sample(j), frame_to_sample(j+1))`, as the plan counts them.
- **What follows from the clock:**
  - Pictures follow the sound as it is heard; the device's latency is made up for.
  - When the mix is late, sound and pictures stop together.
  - At the end the clock holds and the player pauses on the last frame.
  - Play shows its first frame until that frame's first sample is heard.
  - `frame_at(t)` reads the clock at any instant without changing anything: a display asks for the frame of its next refresh, a test for exact instants a moment ago (the clock remembers the device's last 64 buffers).
  - No half-frame lead: sound and picture meet on the frame's first sample.
- **Backward**, the sound plays reversed. The mix is read forward one block at a time (0.2 s of play, one seek of each lane's stream per block), from further and further back, and each block is reversed sample frame by sample frame. The clock works as it does forward: a beep's first sample is the last of it heard, and right after it the picture moves to the frame before.
- **Other speeds**, both ways, go through the engine's own time stretch (`Stretch`, WSOLA), which keeps the pitch and says where its output came from:
  - The mix is cut into 21 ms fragments that overlap by half in the output, Hann-shaped and added. Fragment *k* would start at *k* hops × speed in the input. It moves by up to 5 ms to where its first half best matches what would have followed the fragment before, so the overlap adds similar sound.
  - Each output block (one hop, 10.4 ms) runs from the middle of one fragment to the middle of the next, and both input positions are known. Every block is marked as it is queued, so the clock follows what is heard. A block blends two fragments a hop × |speed − 1| apart in the input, so a sound may be heard up to half that distance from where its block's marker puts it.
  - The engine has its own stretch because FFmpeg's `atempo` moves its fragments where nothing outside can see, so its output can't drive the clock.
  - At 4× and above, a sound much shorter than a fragment may blend away. Tones, speech and music keep their pitch.

## 5. Audio scrubbing (spec §4.5, decision D42)

- **Frame steps and paused seeks** play the sound of the frame arrived at, while `audio_scrubbing` is on (the default; `set_audio_scrubbing`). That is the frame's samples of the mix (every lane, gains, faders, lane gains on top), faded in and out over 3 ms. Dragging the playhead is a series of paused seeks, so it scrubs too. Muted, nothing plays. While playing, steps don't happen and seeks don't scrub.
- **Always forward:** a step back plays its frame forward.
- **One at a time:** the audio thread holds the newest frame's sound until the one in the ring has (nearly) played out, then queues it whole. So a sound is never cut, and a newer one replaces any still waiting. A held key plays frame after frame; steps faster than that skip to the newest, and nothing piles up. The mix seeks for each frame; stepping on decodes on.
- **`Scrubber`** does the same without a player, for a viewer whose pictures come from elsewhere: `Scrubber::new(rate, channels, output)`, `set_plan` on an edit, `play(frame)` for each step, `set_lane_gain` for a fader being dragged. It mixes on its own thread and plays on its own output.
- **Quiet seeks:** `seek_quietly` moves without the frame's sound, for moves the viewer makes by itself (opening at a remembered frame, after an edit).

## 6. Outputs

`AudioOutput` starts pulling from the engine's `AudioSource` on its own thread (`vidcut-audio-out`) and stops when dropped.

- **`CpalOutput`**: an output device through cpal (ALSA on Linux, which PipeWire and PulseAudio serve; WASAPI on Windows).
  - It plays on the system's default, or on the device the preferences' `player.audio_device` names by cpal's device id (stable across runs; `output_devices` lists them).
  - The device is found by its id exactly as listed, because cpal's own lookup misses ALSA's (gotchas.md). While that device isn't connected, it plays on the default; `PlaysOn` says which device plays.
  - Names can repeat (ALSA's outputs of one card share its name; a mixer's two outputs can have the same name), ids don't. `output_devices` tells them apart by the next line of their description ("Front output / input"), else numbers them.
  - A changed choice reopens the engine on the new device, paused where it was.
  - Samples are `f32` at the plan's rate and channels.
  - The latency is the callback's playback time minus its callback time.
  - The stream lives on its own thread, as it can't move between threads on every platform.
  - Device errors show in `Status::error`.
- **`PacedOutput`**: a stand-in that pulls in real time, each buffer heard a set latency after it is due. It can record what it pulled and when it would be heard.
- **`ManualOutput`**: pulls when a test says, so the test sets the clock.
- **cpal** is Apache-2.0 (with the MIT/Apache `alsa` crate and the MIT `alsa-sys`), compatible with MIT distribution. On Linux it builds against ALSA's headers (`libasound2-dev`).
