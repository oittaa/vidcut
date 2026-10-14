# Stats for nerds

**When:** after VideoToolbox decoding, among the last of the player and GPU work.

A panel over the viewer that shows what the player is doing, like a video site's "stats for nerds". It is a bit hidden: the viewer's right-click menu turns it on. `VIDCUT_DEBUG` says the same facts on stderr.

- **Viewer / frames:** the viewer's size in pixels, frames shown, and how many came late or were skipped.
- **Picture / timeline:** the picture's size and rate, and the timeline's.
- **Codecs:** the video's codec and profile, and the audio's.
- **Decoder:** the one actually used, FFmpeg's and on which GPU (`hevc` on NVDEC, RTX 4080 SUPER), or the CPU and why not the GPU.
- **Picture:** its layout, where it is (memory, CUDA, VA-API, Direct3D 11) and how it got on screen.
- **Color:** the file's matrix, range and transfer, and what the viewer shows.
- **Buffer:** pictures kept ahead and behind, against the memory budgets.
- **Sound:** the device, rate, channels and latency.
- **Renderer:** the backend and adapter.

A request turns it on and off, as for every GUI action, and `player.state` gives the same facts.
