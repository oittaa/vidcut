# Threads: what remains

[threads.md](../decisions/threads.md) records the design that runs: the shared pools, every graph's slices and every frame that stands alone on them, a thread budget for the other decoders by what their thread works for, readers whose decoders close with them and a file left open a moment, one file opened ahead at a time. `scripts/threads.sh` and `VIDCUT_DEBUG`'s `[decoding]` lines measure it (tools.md). The steps below are what is open, in order; each is a pull request of its own, measured with the census before and after.

## 1. A still picture in a file opens as single frames

A still (a PNG or JPEG file, not a sequence) opened as a stream starts frame threads to decode one picture: PNG in a file isn't marked intra-only, so it doesn't decode on the pool, and a still is never read on. Open stills as `Reading::Frames` (one thread, graphs in one slice) wherever `Pictures::Still` is known before the reader opens (`export/video.rs`, `Source::open`). Checked by a `tests/threads.rs` test: a still in a plan starts no thread.

## 2. A cap on CPU long-GOP stream decoders

The interactive decoders are bounded by the players (at most four with the windows reader, and four files left a moment each) and the jobs' by the render queue, except analyses and measured light, one for each request. A cap of four job decoders with frame threads, beyond which one opens with a single thread, is a cheap backstop. Only if the census shows it is needed.

## 3. A cache of decoders

A decoder leased by readers and closed when idle, handed back instead of opened anew, only if a record still shows opens that cost: the files left open a moment and a seek taking a file back from the opener (threads.md §4.1) cover a dragged playhead.

## Not this work

The kept-pictures budget: memory in a long play is mostly the player's kept pictures (its budget is 30 GB).
