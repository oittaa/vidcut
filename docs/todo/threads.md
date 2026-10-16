# Threads: a bounded set that ends with its work

The process starts too many threads, and the ones it starts outlive their work. This is the plan: what runs now, the rules to get to, and the steps in order. Each step is a pull request of its own, measured with the census of step 1 before and after. [threads.md](../decisions/threads.md) is the record of what runs; this file holds what it describes but the code doesn't do yet.

## What runs now

Branch `claude/ffmpeg-threads` (PR 263). A 16-core machine; FFmpeg's automatic thread count is the cores, at most 16.

| Source | Threads | Live while |
|---|---|---|
| The app's own (gui.md §1), the window, the GPU drivers, sound out | 39 at rest (`playback-cmp/*.threads`) | the process |
| A CPU stream decoder (`decode::Reading::Stream`, `VideoReader::open_raw`): FFmpeg's frame threads, count 0 = automatic | 16 | its reader |
| swscale, one set for each `scale` filter in a stream's graph (`pool::graph(Stream)` gives the graph the cores as `nb_threads`; the filter hands that to swscale, which starts its own) | 15 | the graph |
| libdav1d, AV1 on the CPU: a pool of its own from the same count | ~16 | its reader |
| A single-frame reader (`Reading::Frames`: filmstrip tiles, thumbnails, storyboards, frame render): one thread, swscale one | 0 | |
| A GPU decoder (`thread_count = 1`) | 0 | |
| A filter graph's own pool (slices run on `pool.rs`) | 0 | |
| The pools, `vidcut-cpu-N` and `vidcut-cpu-job-N` | 15 + 15 | the process, from first use |
| A player: `vidcut-play-video`, `-play-audio`, `-audio-out` with PipeWire's, `-play-windows` while it follows, `-scrub` | 3 to 8 | the player |
| An export: `vidcut-open`, `vidcut-encode`, the encoder's own | 2 + the encoder's | the job |
| Per request: `vidcut-task`, `-make`, `-measure`, `-ask`, `-ipc-conn`; `-two-up-close`, `-viewer-close`, `vidcut frees imports` | 1 each | the request |

**Who opens CPU stream decoders,** 31 threads each on the CPU path (one on the GPU's): the viewer's player (the current piece's layers, and the next file opened ahead), its windows reader while it follows, each two-up cell, the export job (the same, and its file ahead), proxies, analyses (`vidcut-measure`), measured light (and 2 to 4 measuring threads).

**What the logs in `$VIDCUT_TEST_DATA` show** (builds of master, Oct 16):
- `leak-samples.log`: a 4K PNG sequence on the CPU, the filmstrip reading and an export. 148, then 298, then 268 threads, flat for 14 minutes: the four tile workers' idle readers held 31 each (`tiles=124`), the player's 32 (`play=32`); 130 once the tile readers closed; 2 at exit. (RSS went 4.5 to 32 GB meanwhile: the kept-pictures budget is 30 GB; that is the memory work, not this.)
- `playback-cmp/*.threads`: 39 at rest; 84, then 129 with the filmstrip reading, one tile worker at 46. With one thread for single frames: 39 to 40 throughout.
- No log shows a count that grows without bound. They show threads outliving their work, and 31 for every CPU stream decoder. A census by thread name (step 1) decides whether a scenario grows.

**PR 263, checked against its code:** the pools, `pool::graph` for every graph, `Reading` with one thread for single frames: built, and sound. Stated in threads.md but not built: §4, the decode layer (leases, the cache, `vidcut-close`, idle decoders closing, the caps, frames that stand alone decoded on the pool); §3's "the player's first" (the pool's queue is oldest-first); §2's rows for `vidcut-close` and the caps. Its numbers come from `player_load` (release build, the real player) and weren't rerun.

## Rules

1. **One set of frame threads working at a time in each class, and the class's pool.** Interactive: the normal pool and the playing player's decoder. Jobs: the jobs' pool, one job's decoder and its encoder, at the lowest priority, which takes no core from the interactive class. Frames that stand alone take no frame threads at all (step 4): the pool is their only worker set.
2. **A thread lives with its work.** A decoder closes with its reader. A reader that idles closes within 10 s, unless its next frame is expected: the player's current piece and the file ahead.
3. **Counted and seen.** Every decoder open is counted, with its owner thread and its thread count, under `VIDCUT_DEBUG`.
4. **Nothing starts a thread for one call, one frame or one file.**

## Steps

### 1. See it

`scripts/threads.sh` names the owners of a process's threads once a second, and the `[decoding]` lines say each reader's thread count and how many readers are open (tools.md). What remains is the record: the scenarios below, on this branch, with the script and `VIDCUT_DEBUG`, about ten minutes: at rest 30 s; play the PNG timeline 60 s; stop, idle 60 s; scrub and step 30 s; the two-up (Dynamic Trim) 30 s, close it, idle 30 s; an export while playing; the export alone; idle 60 s after it. The record says which scenario grows, if any, who owns what sits, and what threads the encoders get in an export (`export/encode.rs` sets no thread count; gotchas.md says FFmpeg's default is one thread, which libx264 takes as its count).

### 2. threads.md says what runs

Cut §4, §3's "the player's first" and §2's `vidcut-close` and caps rows into this file until they are built (AGENTS.md rule 11). PR 263 can then merge: its built parts hold. `decode.rs`'s `open_raw` and `rewind` are also touched by the kept-pictures branches (`claude/kept-pools`, `claude/pooled-frames-own`): rebase on whichever lands first.

### 3. Thread counts from one budget

In `open_raw` and `pool::graph`, the count a decoder opens with and its graph's `nb_threads` come from one place, `pool::threads_for(reading, class)`:
- Interactive stream decoders (the player, the two-up, the windows reader): the cores, as now.
- Jobs' stream decoders: half the cores. A job's decoder works beside its encoder, and 16 frame threads hold 16 frames in flight (640 MB of 4K 16-bit PNG) that a job never needs. The file opened ahead takes the same count and idles after its first frame.
- A stream graph's `nb_threads`, so swscale's set, follows its decoder's count.
- Encoders: `threads` set to the cores for a job unless the preset sets it (rule 10: a `threads` the user set reaches the encoder as it is). Step 1 shows what they get now.
- Measured by `player_load`'s job frames a second and peak memory, and an export's wall time.

### 4. Frames that stand alone decode on the pool (threads.md §4.2 as written)

The biggest change and the biggest win for PNG sequences, ProRes, DNx, MJPEG and the other intra-only codecs: no frame threads at all. The reader demuxes on its thread and queues each packet as a task on its pool; each pool thread keeps a one-thread decoder for each kind of stream it has decoded (keyed by the stream's parameters, closed after ten seconds unused); frames are collected in the stream's order; a seek drops the tasks not started. In flight: as many frames as the pool has threads for a stream, one for a single-frame reader. Memory in flight is the same as frame threads hold, but once for the process, not once for each reader.
- Why it is right: intra-only frames don't depend on each other, so a pool of one-thread decoders gives everything frame threads give, and the pool is shared and sized to the cores.
- `pool.rs` grows a second kind of work beside slices: a task that returns a frame. The player's first in the normal pool only if step 1's record shows a frame render or the two-up delaying it; otherwise oldest-first stays.
- Checked by `tests/threads.rs` (a stream of PNG frames starts no thread once the pool runs), `player_load`'s PNG frames a second at least what the CPU does now (about 40 fps), and `tests/player.rs`'s barcodes unchanged.

### 5. Decoders are counted, and close when idle (threads.md §4.1, the part worth building)

- A registry in `decode.rs`: every `VideoReader` enters on open (file, reading, thread count, owner thread) and leaves on drop; listed under `VIDCUT_DEBUG`, and the `[decoding]` lines of step 1 read from it.
- Idle close: a tile worker drops its reader ten seconds after its last job (the queue's wait gets a timeout). Thumbnails' and storyboards' readers are per call already; the windows reader ends with following; `VideoFrames` closes the files no later piece reads as each piece starts.
- The cache and leases (an idle decoder handed back instead of opened anew: fewer opens on seeks that rewind, on the two-up opening and closing, on pieces changing files) only if step 1's record shows opens and closes that cost: each open of a CPU stream decoder starts 31 threads and each close ends them.

### 6. A cap on CPU long-GOP stream decoders (threads.md §4.3)

After steps 3 to 5 the interactive decoders are bounded by the players (at most four with the windows reader) and the jobs' by the render queue, except analyses and measured light, one for each request. A cap of four job decoders with frame threads, beyond which one opens with a single thread, is a cheap backstop. Last, and only if the census shows it is needed.

## Not this work

The kept-pictures budget and the decoders' pools ([kept-pictures-pools.md](kept-pictures-pools.md)): the memory work on the branches named in step 2.
