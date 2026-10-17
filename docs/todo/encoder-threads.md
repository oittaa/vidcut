# Export encoders on one thread

vidcut's CPU exports to x265, VP9 (libvpx), AV1 through libaom or rav1e, ProRes and DNxHR encode on one thread: `export/encode.rs` sets no thread count, FFmpeg's default `thread_count` is 1, and each of those encoders takes it as its own count (gotchas.md, "FFmpeg's encoders use one thread unless given more"). Only x264, SVT-AV1 and Kvazaar take the cores by themselves. Proxies already give ProRes and DNxHR frame threads (`proxy.rs`).

**What to do:** a job's encoder takes the cores (`pool::threads()`), at the job's lowest priority, where the encoder uses the count: x265's frame threads, libvpx's and libaom's `g_threads` (with `row-mt` already set), rav1e's and OpenH264's thread count, ProRes's and DNxHR's slice or frame threads. No setting of the user's is touched: the export settings, presets and `--set` have no `threads` key (AGENTS.md rule 10 would otherwise apply).

**Measure** an export's wall time before and after for each encoder, with `passes` (tools.md) or an export of `test.vidcut`, and put every run in the pull request. threads.md §2's export row and gotchas.md say what encoders get afterwards.
