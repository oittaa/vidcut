# The player engine: vidcut's own playback, headless

**Status:** accepted (October 2026). The GUI's viewer plays through it, one engine for the source and the timeline ([preview.md](preview.md)); this record is the engine itself, tested headless.

**Evidence** (all rerunnable, no display):
- [`crates/vidcut-av/tests/player.rs`](../../crates/vidcut-av/tests/player.rs): pictures against `frame render` at seeks and steps both ways across cuts, files, codecs, black, stills and image sequences; the YUV pictures, and their conversion against `frame render`'s to the byte; playing shows every frame in order; the beep lands on its sample; pictures against beeps through a device with latency; 2×, reverse; faders and mute while playing; calls that return at once; a frame of the plan played next, prepared and held whatever the budget, shown at once.
- [`tests/player_sound.rs`](../../crates/vidcut-av/tests/player_sound.rs): backward sound reversed to the sample; pictures with the sound through a device with latency at −1×, 2×, 0.5× and −2×; other speeds both ways keeping the pitch; audio scrubbing: steps play their frame's sound, held steps don't pile up, the `Scrubber` alone.
- [`tests/player_decodes.rs`](../../crates/vidcut-av/tests/player_decodes.rs): what is decoded, playing both ways, across an edit, for seeks in a row, and after switching to a prepared plan (nothing).
- [`tests/player_media.rs`](../../crates/vidcut-av/tests/player_media.rs): numbers on the real 1080p30 and 2160p60 clips, on the CPU and with `--features gpu-tests` on NVDEC too (§7), and what one player for a viewer's source and timeline costs in its one budget; `VIDCUT_PLAYER_MEDIA` measures other files, `VIDCUT_PLAYER_BUDGET_MB` another budget.
- [`tests/player_gpu.rs`](../../crates/vidcut-av/tests/player_gpu.rs) (`--features vidcut-av/gpu-tests`): pictures decoded on each GPU decoder the machine has are the CPU's (§3), and NVDEC's kept in video memory are the ones copied down.
- [`tests/player_device.rs`](../../crates/vidcut-av/tests/player_device.rs) (`--features vidcut-av/audio-tests`): the system's audio device runs the clock (it plays silence).

## 1. Threads

| Thread | Does | Never |
|---|---|---|
| **The caller** (the live session, which requests; the viewer thread, which presents) | sends requests, which return at once; `present` looks a picture up | decodes or waits |
| **Decode** (`vidcut-play-video`) | the picture of the frame the viewer is at, then pictures ahead of it | touches the device |
| **Audio** (`vidcut-play-audio`) | mixes the plan's lanes into the ring; paused, a step's frame | waits on the device |
| **Device callback** (the output's thread) | copies from the ring, notes when it will be heard | locks, allocates or decodes |
| **Openers** (`vidcut-open`, the export's, one per reader) | open the next file ahead | |
| **Scrubber** (`vidcut-scrub`, only with a `Scrubber`) | mixes the frame asked for | |

vidcut-av starts them as it starts its other threads (`std::thread::Builder`, named `vidcut-…`). Someone watches what they make, so they keep their priority: the app's own rule (`threads::spawn`, gui.md §9) gives interactive work no other.

## 2. Requests: one wanted state, merged and cancelled

- **Wanted state.** One mutex holds what is wanted: the plan, the mode (paused at a frame, or playing from a sample at a speed), lane gains, the mute and audio scrubbing. A request changes it and wakes both threads, which read the newest. Requests that arrive while a thread works merge into the last: nothing queues.
- **Position.** Paused, the frame asked for; playing, the clock (§4). `step` pauses where the viewer is and moves; `play` while playing goes on from the sample heard now at the new speed; `play` forward from the last frame starts over from the first (backward from the first, from the last).
- **Cancelling** (the decode thread's flag is `vidcut_av::cancel`'s, which the decoders check). When the new frame's picture isn't kept, the decode in progress is cancelled if it was only decoding ahead, if the player is or was playing, or if it is a paused seek that has run for a second. Otherwise the paused seek finishes and the newest runs next, so a held key moves the picture at the speed frames decode and the last request lands exactly (gui.md §4).
- **After a cancel** the reader forgets where it stands (the next piece of the file seeks), and the file stays open.

## 3. Pictures: decoded YUV, for a shader

- **Picked by the export's reader** (`export::video::VideoFrames` in its `decoded` mode): the sampling rule (export.md §2), deinterlacing, stills and image sequences, so each is the frame export shows. The tests compare every picture's barcode with `frame render`'s.
- **As decoded.** A picture is the decoded frame, held by reference: its own size, its own colors, not turned, in a layout a display takes: 8-bit 4:2:0 (planar, or NV12 as hardware decoders give it) or P010 for more bits. Others are brought to the nearest at their own size (`Relayout`): 4:2:2, 4:4:4 and 10-bit planar are resampled to 4:2:0 keeping their matrix and range (ProRes 4:2:2 10-bit becomes P010); RGB images are coded BT.709 video range. Black is the plan's size in 8-bit 4:2:0. Nothing copies a picture's pixels after the decoder.
- **The display does the rest**, on the GPU: fitting into the viewer as export fits (`Picture::placement`), turning (`rotation`, the display matrix), and YUV to BT.709 RGB from `Picture::colors` (matrix, range, HDR), which are `vidcut_av::color`'s reading of the frame: its tags, or for untagged pictures BT.709 when HD and BT.601 otherwise, BT.2020 when HDR. `Picture::to_rgba` is that whole path on the CPU with the export's own filters: what `frame render` gives (the tests find it equal to the byte for 8-bit sources) and what a shader must match.
- **Decoded on the GPU** when there is one (`Config::hardware`, `set_hardware`; preview.md §3): every stream the GPU decodes (H.264, HEVC, AV1, VP9 where it has them; `VideoReader::open_gpu_or_cpu`), on `Config::gpu` or the best decoder there is (`hw::GpuDecoder::best`: NVDEC, VA-API, Direct3D 11), the rest on the CPU. NVDEC's frames, NV12 or P010, stay in video memory for a display that shows them from there (`Config::gpu_pictures`, `Memory::Cuda`; preview.md §3), and are copied down to memory at their own size with their properties otherwise; the other GPUs' always are, as they are decoded. FFmpeg copies each out of the decoder's few surfaces into a frame of its own, so the store keeps as many as the budget holds. A switch of decoder decodes the pictures anew. Each picture says what decoded it (`Picture::decoder`), and `Status::decoder` is the one shown last.
- **GPU surfaces.** `Picture::memory` and `layout` read a hardware frame's format and frames context (CUDA, VAAPI, Vulkan), and `bytes` what its layout takes, so the budget holds video memory as it holds memory; the engine never reads pixels. `copied_down` (and `to_rgba`, through it) brings one to memory, for a display that can't show it from there and for tests.
- **Kept by what they show:** the file, its origin, still or sequence, the source position and the rate (black by the plan's size). Pictures survive edits that move them (a ripple), and seeking to a kept one shows it without waiting for the decode thread. Memory budgets hold them (`Config::budget`), one for pictures in memory and one for pictures in video memory, each counted where it lives; over one, that kind's least recently kept or shown go first, never the prepared picture. `Budget::automatic` is a viewer's: the larger of 1 GB and half the computer's memory (POSIX `sysconf`), and half the GPU's memory once the decode thread opens it; `DEFAULT_BUDGET` (1 GB of each) is for tests and tools. `set_budget` changes them while it plays. A picture takes what its layout does: 3.1 MB for 1080p, 12.4 MB for 2160p 4:2:0; counts below are in pictures like the latest.
- **Forward**, the reader decodes on from frame to frame, and the next file opens ahead on the export's opener thread. Frames the clock has already passed are decoded and not kept. A reader far behind (more than a second) seeks to where the clock will be once the seek is done (the recent seek time × speed) and goes on from there while the clock comes.
- **Ahead:** a quarter of a second × speed while playing (at most a quarter of the budget); paused, three frames when the reader stands right there.
- **Back:** a window from a keyframe up to the frame, kept (the two-up renderer's rule, preview.md §9), at most half the budget. It reaches back over another keyframe only to hold that GOP whole: started between two keyframes, the frames before it decode for nothing.
- **Playing backward**, a frame with nothing kept there decodes from its own keyframe, so it shows after one GOP's decoding, not two. Behind the window shown two more wait where they fit, and once one is left the one before it decodes. The first behind is the nearest, from its own keyframe, as the playhead may be close; the second holds at least two seconds where keyframes are close and decodes once it fits in the budget beside the frames still to be shown. Before a window decodes, those count as looked at, so the frames shown go first when the budget is full.
- **Around a paused playhead**, whichever way it last went: at least two seconds behind it (at most half the budget), decoded while nothing else is wanted, nearest first, so the first steps back find their pictures. A window goes from the keyframe of the latest frame missing up to it: in video memory, all of it at once; in memory, half a second of it at a time (each kept picture there takes new memory, and a 10-bit one a conversion), so a step right after a seek waits for half a second's frames, not the GOP's. A step back into the window being decoded waits for it rather than start it again (`Busy::Window`); a step past what is kept decodes such a chunk itself.
- **A seek** decodes from the keyframe before its frame and keeps what it decodes on its way: in video memory all of it (kept, those cost nothing more); in memory the last half second, as each kept there takes new memory and a 10-bit one a conversion (keeping the whole way slowed 4K seeks on the CPU by up to 0.23 s; half a second costs 5–110 ms). The steps back right after it find their pictures; the rest behind comes while nothing else is wanted. Finding a window's keyframe takes one seek and one decoded frame, and the window decodes on from there (`VideoReader::from_keyframe`), however far into the GOP it starts.
- **Edits.** `set_plan` keeps the position, the open file (the reader reads the new plan's pieces: a piece of the open file decodes on or seeks) and the kept pictures. The sample rate and channels must stay (the device was opened for them): another is refused before anything changes, and the same plan again changes nothing, so neither cancels a decode. Playing, the sound goes on from the same time; at another frame rate the picture shown stays until the new plan's is decoded.
- **The plan played next.** `prepare(plan, frame)` names a frame of another plan, such as the timeline's while a viewer shows the source. Paused with nothing else to decode, the decode thread decodes it on a reader of its own, made for that picture, so the reader it plays with stays where it stands; a new request cancels it as it cancels decoding ahead. The store holds its picture whatever the budget until another is prepared (`None` lets it go), `Status::prepared` says whether it is decoded, and `on_picture` is called when it lands. `set_plan_at(plan, frame)` then plays that plan from that frame in one change (nothing decodes for the frame in between): the picture is there, so the next `present` shows it. A frame not decoded yet keeps the picture shown before until its own lands. Which way the viewer last went is forgotten with a new plan: steps back in one say nothing of the other, so no window behind decodes there.

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
  - `frame_at(t)` reads the clock at any instant without changing anything: a display asks for the frame of its next refresh, a test for exact instants a moment ago (the clock remembers the device's last 64 buffers).
  - No half-frame lead: the sound and the picture meet on the frame's first sample.
- **Backward** the sound plays reversed: the mix is read forward a block at a time (0.2 s of play, one seek of each lane's stream a block), from further and further back, and each block is reversed a sample frame at a time. It is the clock as forward sound is: a beep's first sample is the last of it heard, and the picture moves to the frame before right after it.
- **Other speeds**, both ways, go through the engine's own time stretch (`Stretch`, WSOLA), which keeps the pitch and says where its output came from:
  - The mix is cut into fragments of 21 ms that overlap by half in the output, Hann-shaped and added. Fragment *k* would start *k* hops × speed into the input; it moves by up to 5 ms to where its first half best matches what would have followed the fragment before, so the overlap adds alike sound.
  - Each output block (a hop, 10.4 ms) runs from the middle of one fragment to the middle of the next, and both input positions are known: every block is marked as it is queued, so the clock follows what is heard. A block blends two fragments a hop × |speed − 1| apart in the input, so a sound may be heard up to half that from where its block's mark puts it: 2.6 ms of play at 2×, 3.9 ms at 4×.
  - FFmpeg's `atempo` can't be the clock: it moves its fragments by up to 21 ms where nothing outside can see, and its output drifts from the input by 10–20 ms over a few seconds.
  - At 4× and above a sound much shorter than a fragment may blend away (a 10 ms beep lasts 2.5 ms at 4×); tones, speech and music keep their pitch (a 1 kHz tone stays within 2 Hz from 0.5× to 8×, both ways).

## 5. Audio scrubbing (spec §4.5, decision D42)

- **Frame steps and paused seeks** play the sound of the frame arrived at, when `audio_scrubbing` is on (the default; `set_audio_scrubbing`): its samples of the mix (every lane, gains, faders, lane gains on top), faded in and out over 3 ms. A drag of the playhead is paused seeks, so it scrubs too. Muted, nothing plays; playing, steps don't happen and seeks don't scrub.
- **Forward, either way:** a step back plays its frame forward.
- **One at a time:** the audio thread keeps the newest frame's sound until the one in the ring has (all but) played out, then queues it whole, so a sound is never cut and a newer one replaces any still waiting. Held keys play frame after frame, steps faster than that skip to the newest, and nothing piles up. The mix seeks per frame; stepping on decodes on.
- **`Scrubber`** does the same without a player, for a viewer whose pictures come from elsewhere: `Scrubber::new(rate, channels, output)`, `set_plan` on an edit, `play(frame)` for each step, `set_lane_gain` for a fader being dragged. Its own thread mixes; its own output plays.
- **Quiet seeks:** `seek_quietly` moves without the frame's sound, for moves the viewer makes by itself (opening at a remembered frame, after an edit).

## 6. Outputs

`AudioOutput` starts pulling from the engine's `AudioSource` on a thread of its own and stops when dropped.
- **`CpalOutput`**: the system's default device through cpal (ALSA on Linux, which PipeWire and PulseAudio serve; WASAPI on Windows), `f32` at the plan's rate and channels. The latency is the callback's playback time minus its callback time: 37 ms through PipeWire here. The stream lives on its own thread, as it can't move between threads on every platform. Device errors show in `Status::error`.
- **`PacedOutput`**: a stand-in that pulls in real time, each buffer heard a set latency after it is due; it can record what it pulled and when it would be heard.
- **`ManualOutput`**: pulls when a test says, so the test sets the clock.
- **cpal** is Apache-2.0 (with the MIT/Apache `alsa` crate and the MIT `alsa-sys`): compatible with MIT distribution. It builds against ALSA's headers, `libasound2-dev`.

## 7. Measured

On the dev machine (16 threads), decoding on the CPU; YUV pictures at the clips' own size; a display presenting at 60 Hz.

| What | Result |
|---|---|
| Picture against sound through a device with 40 ms latency: when the clock moved to each beep's frame after the beep was heard, read at exact instants half a millisecond apart (`Player::frame_at`), so a test thread run late measures the same (the tests allow −1 to +8 ms, the bound a polling display needed) | 1×: 0.3–0.5 ms; −1× (reversed sound): 0.1–0.4 ms; 2×: 2.6–3.1 ms; 0.5×: 0.2–0.4 ms; −2×: 0.6–0.9 ms, also with the CPU busy with other decoding |
| The beep through the manual output, forward and reversed | on its sample, ±1 (reversed: the whole beep, sample for sample) |
| Other speeds, both ways (0.5× to 8×) | a 1 kHz tone at 998–1000 Hz; every beep heard up to 2×; the sound as long as the speed makes it (within 20 ms) |
| Requests while decoding 1080p | the slowest call under 0.01 ms |
| Decoded playing 0→150, an edit on the way | 149–154 frames |
| Decoded playing 299→0 backward, keyframes 120 apart | 306 frames |
| 1080p30 H.264 pictures | 3.1 MB each; a window back holds 114 at the default 1 GB (57 at 512 MB) |
| 1080p30: seek to picture | median 96–110 ms, max 209–221 ms |
| 1080p30: 5 s forward / 3 s backward | 150 of 150 shown, 0.1 cores / 90 of 91, 0.4 cores |
| 2160p60 H.264 pictures (keyframes 4 s apart) | 12.4 MB each; a window back holds 28 at the default 1 GB (14 at 512 MB) |
| 2160p60: seek to picture | median 188 ms, max 373 ms |
| 2160p60: 5 s forward | 298 of 300 shown, none late, 0.4 cores |
| 2160p60: 3 s backward, at the default 1 GB | 177–178 of 180–181 shown, none late, 3.9–4.6 cores |
| 2160p60: 3 s backward at 512 MB | 153–159 of 180 shown, 19–24 presents late, 7.1–7.2 cores |
| System device (PipeWire) | 37 ms latency; the picture moves 81 ms after play |
| NVDEC (`--features gpu-tests`): 1080p30 H.264, seek to picture / 5 s forward / 3 s backward | median 296 ms / 150 of 150 shown, 0.0 cores / 90 of 91, 0.1 cores |
| NVDEC: 2160p60 H.264 | median 788 ms / 298 of 300, 0.1 cores / 56 of 181 shown, 123 presents late |
| 2160p60 10-bit HEVC (bbb, NVENC, 24.9 MB P010 pictures), CPU / NVDEC | seeks median 238 / 108 ms; forward 2.0 / 0.1 cores, none late; backward 23 / 84 of 181 shown |
| 2160p60 10-bit AV1, CPU / NVDEC | seeks median 208 / 115 ms; forward 1.6 / 0.1 cores, none late; backward 55 / 83 of 181 shown |
| One player for a viewer's source and timeline (CPU, 1080p30 / 2160p60): switching to the prepared frame | 0.03–0.7 ms; to a frame not decoded yet, a seek: 60–68 / 151–166 ms |
| The timeline after the source played 8 s elsewhere in the file, at 1 GB | first step back 52 / 169 ms (a window decoded again; with nothing played between, 0 ms), the next 39 median 0.0 ms; 3 s backward 90 of 91 / 179 of 181 shown, as with nothing played between |
| The same at 2 GB | first step back 34 / 171 ms; 3 s backward 90 of 91 / 179 of 181 shown |
| The same on NVDEC, 1 GB (1080p30 / 2160p60) | switching to the prepared frame 0.03–0.05 ms, to one not decoded yet 560–567 / 893 ms; after the source played 8 s, the first step back 429 / 1,209 ms (with nothing played between, 0 ms) |

Backward 4K in a 4 s GOP decodes most windows from the keyframe again, so the budget decides it: 14 pictures a window (512 MB) take 7.1 cores and drop frames; 28 (1 GB, the default) take 4.3 and drop none. A smaller budget suits smaller sources.

One budget for a viewer's source and timeline costs one window at 1 GB: 8 s of 4K play goes through 480 pictures (6 GB), so the windows of the one not shown go, and its first step back after a switch decodes one again. The prepared picture stays, and a switch needs no more. 2 GB kept no more of the timeline than 1 GB once the source had played. On NVDEC that window takes as long as a first step back there: 1.2 s for 4K H.264. The automatic budget (half the memory: 29 GB on this machine, 8 GB of video memory) keeps far more than 8 s of 4K. Steps back measured in the GUI are in preview.md §8.

NVDEC's speed against the CPU, and why the GPU is still the default: preview.md §3. The GUI's numbers are in preview.md §8.

## 8. Not yet

- **Other GPUs' frames in video memory** (own-player.md): VA-API's and Direct3D 11's, or Vulkan Video's, which would need no copy at all.
- **Sound:** choosing the device, resampling for a device that won't open at the plan's rate.
