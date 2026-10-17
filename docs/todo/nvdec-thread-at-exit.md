# NVDEC's worker thread outlives its decoders

**When:** next time on a Linux desktop with an NVIDIA GPU, with the GUI's exit stages (`threads::ending`) and `gdb` as tools.md says.

After a window closed while NVDEC decoded, a thread the driver started from the decode thread (named `vidcut-play-video` after it) stayed, waiting in libcuda's `pthread_cond_timedwait` in a loop, after every reader of vidcut's had closed and every picture was freed. libnvcuvid's exit handler joined it forever, so the GUI ends with `_exit` (gotchas.md). Find what the thread waits for: a decoder FFmpeg's `nvdec` keeps alive through a frame still referenced (its decoders are freed with the last frame of theirs), a CUDA stream or event of `cuda::share` or `cuda::keep` not destroyed, or the driver's own. A reader closed while decoding is the case to try: close the window, or seek away, while an NVDEC piece plays.
