# The player engine: vidcut's own playback, headless

**Status:** accepted (October 2026). Step 3 of [own-player.md](../todo/own-player.md): the engine runs and is tested headless; the GUI still previews with mpv ([preview.md](preview.md)).

**Evidence** (all rerunnable, no display):
- [`crates/vidcut-av/tests/player.rs`](../../crates/vidcut-av/tests/player.rs): pictures against `frame render` at seeks and steps both ways across cuts, files, codecs, black, stills and image sequences; playing shows every frame in order; the beep lands on its sample; pictures against beeps through a device with latency; 2×, reverse; faders and mute while playing; calls that return at once.
- [`tests/player_decodes.rs`](../../crates/vidcut-av/tests/player_decodes.rs): what is decoded, playing both ways, across an edit, and for seeks in a row.
- [`tests/player_media.rs`](../../crates/vidcut-av/tests/player_media.rs): numbers on the real 1080p30 and 2160p60 clips (§6).
- [`tests/player_device.rs`](../../crates/vidcut-av/tests/player_device.rs) (`--features vidcut-av/audio-tests`): the system's audio device runs the clock (it plays silence).

## 1. Threads

| Thread | Does | Never |
|---|---|---|
| **The caller** (later the live session and the renderer) | sends requests, which return at once; `present` looks a picture up | decodes or waits |
| **Decode** (`vidcut-play-video`) | the picture of the frame the viewer is at, then pictures ahead of it | touches the device |
| **Audio** (`vidcut-play-audio`) | mixes the plan's lanes into the ring | waits on the device |
| **Device callback** (the output's thread) | copies from the ring, notes when it will be heard | locks, allocates or decodes |
| **Openers** (`vidcut-open`, the export's, one per reader) | open the next file ahead | |

vidcut-av starts them as it starts its other threads (`std::thread::Builder`, named `vidcut-…`). Someone watches what they make, so they keep their priority: the app's own rule (`threads::spawn`, gui.md §9) gives interactive work no other.

## 2. Requests: one wanted state, merged and cancelled

- **Wanted state.** One mutex holds what is wanted: the plan, the picture size, the mode (paused at a frame, or playing from a sample at a speed), lane gains and the mute. A request changes it and wakes both threads, which read the newest. Requests that arrive while a thread works merge into the last: nothing queues.
- **Position.** Paused, the frame asked for; playing, the clock (§4). `step` pauses where the viewer is and moves; `play` while playing goes on from the sample heard now at the new speed; `play` forward from the last frame starts over from the first (backward from the first, from the last).
- **Cancelling** (the decode thread's flag is `vidcut_av::cancel`'s, which the decoders check). When the new frame's picture isn't kept, the decode in progress is cancelled if it was only decoding ahead, if the player is or was playing, or if it is a paused seek that has run for a second. Otherwise the paused seek finishes and the newest runs next, so a held key moves the picture at the speed frames decode and the last request lands exactly (gui.md §4).
- **After a cancel** the reader forgets where it stands (the next piece of the file seeks), and the file stays open.

## 3. Pictures

- **Made by the export's reader** (`export::video::VideoFrames`): the sampling rule (export.md §2), fitting and padding, colors, rotation, deinterlacing, stills and image sequences. The tests compare every picture's barcode with `frame render`'s.
- **Output.** A size and pixel format (`Output`): packed RGBA for drawing with the CPU, or YUV for a shader. `set_output` when the viewer is resized.
- **No copies.** A picture is an FFmpeg frame held by reference; scaling into the output is the one pass over its pixels. `Picture::memory` says where they are: `Cpu` now; a CUDA, VAAPI or Vulkan surface is the same frame with a hardware format and context, which the engine passes on without reading. GPU frames change the reader and the display, not the engine.
- **Kept by what they show:** the file, its origin, still or sequence, the source position, the rate and the output. Pictures survive edits that move them (a ripple), and seeking to a kept one shows it without waiting for the decode thread. A memory budget (512 MB by default) holds them; the least recently kept or shown go first.
- **Forward**, the reader decodes on from frame to frame, and the next file opens ahead on the export's opener thread. Frames the clock has already passed are decoded but not scaled. A reader far behind (more than a second) seeks to where the clock will be once the seek is done (the recent seek time × speed) and goes on from there while the clock comes.
- **Ahead:** a quarter of a second × speed while playing (at most a quarter of the budget); paused, three frames when the reader stands right there.
- **Back:** a window from a keyframe up to the frame, kept (the paused renderer's rule, preview.md §13): at least two seconds where keyframes are close, at most a third of the budget. Playing backward, the window before decodes while this one is shown; paused after a step back, one window behind.
- **Edits.** `set_plan` keeps the position, the open file (the reader reads the new plan's pieces: a piece of the open file decodes on or seeks) and the kept pictures. The sample rate and channels must stay: the device was opened for them.

## 4. Sound, and the clock

- **The mix** is the export's (`export::audio::AudioMix`): every lane, clip gains and faders as the plan has them, de-click fades at cuts. The audio thread mixes 10 ms at a time into a ring of a second, keeping a tenth of a second queued. A chunk is never dropped: the thread waits for room.
- **Live faders.** `set_lane_gain` puts a gain on top of a lane's (the Mixer's fader while dragged), heard within a tenth of a second plus the device's latency; no ZeroMQ. A new plan drops them: by then the saved level is in the plan.
- **Mute** writes silence; the clock runs on.
- **The ring** is lock-free, one writer and one reader. Positions count sample frames since the player started and never wrap. Seeks, pauses, new plans and new speeds drop what was queued.
- **Markers** say where each run of ring samples is on the timeline: from ring position *c*, timeline sample *s*, moving *v* timeline samples per ring frame (negative backward), for play request *e*. The end of the plan (or its start, backward) gets one that holds.
- **The device's callback** copies from the ring (silence when it is empty) and records, lock-free, the ring position of its first sample, how many ring frames it took and when the first is heard: now plus the output's latency.
- **The clock** at time *t*: the newest buffer heard by *t*, the ring position as far into it as *t* reaches (no further than the frames it held), and through the markers a timeline sample. The viewer shows the frame holding that sample: frame *j* spans samples `[frame_to_sample(j), frame_to_sample(j+1))`, as the plan counts them.
- **What follows:**
  - Pictures follow the sound as it is heard, the device's latency made up for.
  - When the mix is late, sound and pictures stop together.
  - At the end the clock holds and the player pauses on the last frame.
  - Play starts by showing its first frame until that frame's first sample is heard.
  - No half-frame lead: mpv's preview sound leads its picture by half a frame (preview.md §10); the engine has no EDL and no bias.
- **Speeds.** Other forward speeds go through FFmpeg's `atempo`, chained in stages between 0.5 and 2 so it blends rather than skips; the pitch stays. Backward, the device plays silence, which runs the clock.

## 5. Outputs

`AudioOutput` starts pulling from the engine's `AudioSource` on a thread of its own and stops when dropped.
- **`CpalOutput`**: the system's default device through cpal (ALSA on Linux, which PipeWire and PulseAudio serve), `f32` at the plan's rate and channels. The latency is the callback's playback time minus its callback time: 37 ms through PipeWire here. The stream lives on its own thread, as it can't move between threads on every platform. Device errors show in `Status::error`.
- **`PacedOutput`**: a stand-in that pulls in real time, each buffer heard a set latency after it is due; it can record what it pulled and when it would be heard.
- **`ManualOutput`**: pulls when a test says, so the test sets the clock.
- **cpal** is Apache-2.0 (with the MIT/Apache `alsa` crate and the MIT `alsa-sys`): compatible with MIT distribution. It builds against ALSA's headers, `libasound2-dev`.

## 6. Measured

On the dev machine (16 threads), decoding on the CPU, pictures 1920×1080 RGBA for the real clips and 320×180 for the fixtures.

| What | Result |
|---|---|
| Picture against sound, through a device with 40 ms latency | each beep's frame shown 0.1–2 ms after the beep is heard (the display looks every 2 ms) |
| The beep through the manual output | on its sample, ±1 |
| Requests while decoding 1080p | the slowest call 0.01 ms |
| Decoded playing 0→150, an edit on the way | 149–154 frames |
| Decoded playing 299→0 backward, keyframes 120 apart | 306 frames |
| Decoded for 31 seeks in a row | 14–94 frames (one GOP at most) |
| 1080p30 H.264: seek to picture | median 71 ms, max 208 ms (328 and 722 with other builds running) |
| 1080p30: 5 s forward, presented at 60 Hz | 150 of 150 frames shown, none late, 0.2 cores |
| 1080p30: 3 s backward | 90 of 90 shown, 0.7 cores |
| 2160p60 H.264: seek to picture | median 189–205 ms, max 413 ms |
| 2160p60: 5 s forward | 297 of 300 shown, none late, 1.2 cores |
| 2160p60: 3 s backward | 178 of 181 shown, none late, 6.1 cores |
| System device (PipeWire) | 37 ms latency; the picture moves 81 ms after play (latency, the first frame's 33 ms, the first chunk) |

Backward 4K costs six cores because its windows are small: a third of 512 MB is 20 pictures of 8 MB, so most windows start past their keyframe and decode it again. Smaller pictures (YUV is 3 MB at 1080p) or pictures in GPU memory make them longer.

## 7. Not yet

- **GPU frames** (milestone 2): decoding on NVDEC, VAAPI or Vulkan into the engine's pictures, scaled there (export's `VideoFrames::with_gpu` does it for NVENC), kept in video memory. The CPU path stays for what GPUs don't decode.
- **The display and the GUI** (own-player.md, the wgpu step): pictures as textures, YUV to RGB in shaders matching `vidcut_av::color`, mpv removed. The live session then seeks and presents through the engine (a seek is shown once presented: `Status::presented`), `player.state` reads `Status`, `mixer.preview` calls `set_lane_gain`, `player.mute` calls `set_muted`.
- **Sound:** reversed sound playing backward, sound while scrubbing, choosing the device, resampling for a device that won't open at the plan's rate.
