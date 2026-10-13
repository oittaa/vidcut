# The player engine

vidcut's own playback engine (`vidcut_av::player`), tested headless. The GUI's viewer plays the source and the timeline through it ([preview.md](preview.md)).

Tests: `crates/vidcut-av/tests/player*.rs`; `player_gpu.rs` needs `--features vidcut-av/gpu-tests`, `player_device.rs` `--features vidcut-av/audio-tests`.

## 1. Threads

| Thread | Does | Never |
|---|---|---|
| **Caller** (the session; the viewer thread presents) | sends requests, which return at once; `present` looks up a picture | decodes or waits |
| **Decode** (`vidcut-play-video`) | decodes the viewer's frame, then ahead of it | touches the device |
| **Audio** (`vidcut-play-audio`) | mixes into the ring | waits on the device |
| **Device callback** (the output's thread) | copies from the ring, records when it is heard | locks, allocates or decodes |
| **Openers** (`vidcut-open`, one per reader) | open the next file ahead | |
| **Scrubber** (`vidcut-scrub`, only with a `Scrubber`) | mixes the frame asked for | |

They keep normal priority: playback is interactive work (gui.md §9).

## 2. Requests: one wanted state, merged and cancelled

- **One wanted state.** A mutex holds the plan, the mode (paused at a frame, or playing from a sample at a speed), lane gains, mute and audio scrubbing. A request changes it and wakes both threads, which read the newest state; nothing queues, so requests that arrive while a thread works merge.
- **Position.** Paused, the frame asked for; playing, the clock (§4). `step` moves from where the viewer is; `play` while playing continues from the sample heard now; `play` from the last frame forward starts from the first (backward from the first, from the last).
- **Cancelling** (`vidcut_av::cancel`). When the new frame's picture isn't kept, the decode in progress is cancelled if it was only decoding ahead, if the player is or was playing, or if it is a paused seek older than a second (`PATIENCE`); otherwise it finishes and the newest request runs next (held keys: gui.md §4).

## 3. Pictures: decoded YUV, for a shader

- **Chosen by the export's reader** (`export::video::VideoFrames`, `decoded` mode): its sampling (export.md §2), deinterlacing, stills and image sequences, so each picture is the frame export shows.
- **Kept as decoded**, by reference, at its own size and colors, not rotated: 8-bit 4:2:0 (planar, or NV12 from hardware) or P010. Other layouts become the nearest at their own size (`Relayout`), keeping matrix and range; RGB images become BT.709 video range. Nothing copies pixels after the decoder.
- **The display does the rest on the GPU:** placement (`Picture::placement`), rotation (`Picture::rotation`) and YUV to BT.709 RGB by `Picture::colors` (export.md §4). `Picture::to_rgba` does the same on the CPU with the export's filters, as `frame render` does; a shader must match it.
- **GPU decoding** (`Config::hardware`, `set_hardware`; preview.md §3) uses `Config::gpu`, else `hw::GpuDecoder::best`. With `Config::gpu_pictures`, frames stay in video memory (`Picture::memory`) for a display that shows them from there, else they come down. Another decoder decodes the pictures again; `Picture::decoder` says which decoded one.
- **Keyed by what they show** (file, origin, still or sequence, source position, rate; black by size), so pictures survive edits that move them, and seeking to a kept picture shows it without the decode thread.
- **Budgets** (`Config::budget`; `set_budget` while playing): one for memory and one for video memory, each counting the bytes of its pictures' layouts. Over a budget the least recently kept or shown go first; the prepared picture never goes.
  - `Budget::automatic` (a viewer's): the larger of 1 GB and half the computer's memory; for video memory, half the GPU's once NVDEC opens, else 1 GB, since VA-API and Direct3D 11 don't report theirs.
  - `DEFAULT_BUDGET` (1 GB of each) is for tests and tools.
- **Forward**, the reader decodes on and the next file opens ahead. Frames the clock has passed aren't kept; a reader more than a second behind seeks to where the clock will be once the seek is done. Ahead: a quarter second × speed while playing (at most a quarter of the budget); paused, three frames when the reader is there.
- **Back**, there is nothing to decode on from, so a window from a keyframe up to the frame decodes at once and is kept, at most half the budget: the two-up's rule (preview.md §8). Playing backward, two windows wait behind the one shown where they fit.
- **Paused**, with nothing else wanted, at least two seconds behind the playhead decode (at most half the budget), nearest first, so the first steps back wait for nothing. A seek keeps what it decodes from its keyframe: all of it in video memory, the last half second in memory, where each kept picture takes new memory.
- **Edits.** `set_plan` keeps the position, the open file and the kept pictures; while playing, the sound continues from the same time. A plan with another sample rate or channel count is refused, since the device was opened for them; it and the same plan again change nothing, nor cancel a decode.
- **The plan played next.** `prepare` names a frame of another plan (the timeline's while the viewer shows the source).
  - Paused with nothing else to decode, it decodes on a reader of its own, so the playing reader keeps its position. Its picture stays, whatever the budget, until another is prepared (`None` releases it); `Status::prepared` and `on_picture` say when it lands.
  - `set_plan_at(plan, frame)` plays that plan from that frame in one change; a decoded prepared picture shows at the next `present`.
  - Both plans share one budget, so the other's windows behind may have to decode again after a switch.

## 4. Sound, and the clock

- **The mix** is the export's (`export::audio::AudioMix`): every lane, gains and faders, de-click fades at cuts. The audio thread mixes 10 ms at a time into a lock-free ring of one second, keeping a tenth of a second queued, and waits for room rather than drop a chunk. Seeks, pauses, new plans and new speeds drop what was queued.
- **Live faders.** `set_lane_gain` puts a gain on top of a lane's own (the Mixer's fader while dragged), heard within a tenth of a second plus the device's latency; a new plan, holding the saved level, drops it. **Mute** writes silence; the clock runs on.
- **The clock.** Markers map ring positions to timeline samples. The device callback records, lock-free, where each buffer starts in the ring and when its first sample is heard (now plus the output's latency). The clock maps what is heard at an instant through the markers; the viewer shows the frame holding that sample.
- **What follows:**
  - Pictures follow the sound as heard, the device's latency made up for, and meet it on the frame's first sample.
  - When the mix is late, sound and pictures stop together; at the end the clock holds and the player pauses on the last frame.
  - `frame_at(t)` reads the clock at any instant: a display asks for its next refresh's frame, a test for instants a moment ago.
- **Backward**, the sound plays reversed: the mix is read forward a block at a time (0.2 s of play), from further and further back, and each block is reversed. The clock works as forward.
- **Other speeds**, both ways, go through the engine's own WSOLA time stretch (`Stretch`), which keeps the pitch and says where each output block came from. FFmpeg's `atempo` hides where its fragments move, so it can't drive the clock. At 4× and above, a sound much shorter than a fragment (21 ms) may blend away.

## 5. Audio scrubbing (spec §4.5, decision D42)

- **Steps and paused seeks** (dragging the playhead too) play the frame's sound while `audio_scrubbing` is on (the default; `set_audio_scrubbing`): its samples of the mix, faded over 3 ms, always forward. Not while muted or playing.
- **One at a time:** the newest frame's sound waits until the one in the ring has (nearly) played out, then is queued whole, so no sound is cut and steps faster than that skip to the newest.
- **`Scrubber`** does the same without a player, for a viewer whose pictures come from elsewhere (`new`, `set_plan`, `play(frame)`, `set_lane_gain`), on its own thread and output.
- **`seek_quietly`** moves without the sound, for moves the viewer makes itself (opening at a remembered frame, after an edit).

## 6. Outputs

`AudioOutput` pulls from the engine's `AudioSource` on its own thread (`vidcut-audio-out`) until dropped.

- **`CpalOutput`**: a device through cpal (ALSA on Linux, which PipeWire and PulseAudio serve; WASAPI on Windows), in `f32` at the plan's rate and channels.
  - It plays on the default, or on the device the preferences' `player.audio_device` names by cpal's id (stable across runs; `output_devices` lists them). The id is matched as listed, since cpal's own lookup misses ALSA's (gotchas.md). While that device isn't connected it plays on the default; `PlaysOn` says which.
  - A changed choice reopens the engine on the new device, paused where it was. Device errors show in `Status::error`.
- **`PacedOutput`** pulls in real time, each buffer heard a set latency after it is due, and can record what it pulled. **`ManualOutput`** pulls when a test says, so the test sets the clock.
- **cpal** is Apache-2.0 (with the MIT/Apache `alsa` and MIT `alsa-sys` crates), fine for MIT distribution; on Linux it builds against `libasound2-dev`.
