# Developer tools

**Fixtures:** generate one (cached, prints its path):

```bash
cargo run --release -p vidcut-av --example make_fixture -- hevc10 mkv 3840x2160 60 600
```

`g=GOP` and `b=BFRAMES` set the keyframe spacing and B-frames (H.264 with B-frames has open GOPs). `busy` adds noise around the barcode, new in every frame: at 4K60, 260 to 400 Mbit/s, as heavy to decode as camera footage.

**Project speed:** `scripts/history-speed.sh [EDITS]` (release builds) makes a project of 300 media and 2000 clips in a temporary folder, with `FILL` undo steps (100). It then times EDITS edits, each followed by a command that only reads, and prints the history's size after each.

**Step-back speed:** `scripts/step-back-speed.py [STEPS] [MB…]` (release builds) opens a window on a 4K 10-bit HEVC clip with keyframes 2 s apart. It times STEPS steps back (100), one request each, with `player.video_memory_mb` at each MB given (1024 8192). `CLIP=FILE` times another clip.

**Slow and dropped frames:** `scripts/playback-check.py [--log DIR] [CODEC…]` (`make playback`, release builds) opens a window on 4K60 `busy` AV1, 10-bit HEVC and VP9 clips with keyframes 2 s apart. It plays forward and backward, jumps and plays, steps both ways and runs and steps Dynamic Trim, each once. It prints, for each move, the frames the viewer showed late or skipped, the two-up's frames passed over, and each step's time. It fails when any frame was late, skipped or passed over. `--log` keeps each window's debug log (`VIDCUT_DEBUG`) as `DIR/CODEC.log`.

**GUI tests:** `crates/vidcut/tests/gui_playback.rs` runs whole GUI sessions on 4K60 `busy` clips, decoding on the GPU, one at a time: cargo starts the tests at once, and each waits for the session before it to end (`ONE_GUI`). One session keeps the GPU's video decoder mostly busy, and the gates on the two-up's windows ([playback](design/playback.md#the-two-up)) are the process's, so sessions side by side miss the session's and the tests' time limits.

**Threads:** `scripts/threads.sh [PID] [SECONDS]` (Linux) prints once a second the process's threads by name, with their total and the resident memory: `vidcut-play-vid=32` is the player's decode thread and the 31 FFmpeg started from it, which carry its name (Linux keeps 15 characters of a name). PID is the newest `vidcut` by default. `VIDCUT_DEBUG`'s `[decoding]` lines say each reader's thread count as it opens, and how many readers are open as one opens or closes.

**GPU encoders:** `cargo run -p vidcut-av --example encoders` test-encodes every GPU encoder (NVENC, VAAPI, AMF, QSV) on every GPU, uncached.

**Other FFmpeg versions:** `scripts/ffmpeg-check.sh [TARGET…]` (Docker) runs `scripts/check.sh full` against the checkout in a container per target, and prints a summary.
- Targets: `lts` (Ubuntu 26.04's FFmpeg 8.0), `release` (the newest release) and `master` (FFmpeg's git master, allowed to fail). Default: `lts release`.
- `--fast` runs `scripts/check.sh`; `-- CMD` runs any command, `-- bash` a shell.
- The containers have no GPU, display or audio device. `scripts/ffmpeg-host.sh [release|master] [-- CMD]` runs a command on this computer, with its GPU, against the target's FFmpeg: copied out of the image to `target/ffmpeg/TARGET/prefix` (once for each image), with `FFMPEG_DIR`, `LD_LIBRARY_PATH` and a cargo target directory of its own (`target/ffmpeg/TARGET/cargo`) set, and a linker wrapper that searches FFmpeg's directory first (alsa-sys puts the system's ahead of it). Its FFmpeg runs with this computer's libraries, Ubuntu 26.04's as in the image. `scripts/ffmpeg-host.sh -- cargo test -p vidcut-av --test vulkan_prores` tests ProRes decoded on Vulkan, which Ubuntu's FFmpeg 8.0 doesn't decode.
- FFmpeg's build (`scripts/ffmpeg/build-ffmpeg.sh`) is a cached image layer. `VIDCUT_FFMPEG_MASTER=COMMIT` pins master.
- Each target's build takes about 5.5 GB, the images about 3 GB. To free them: `docker volume rm $(docker volume ls -q -f name=vidcut-ffmpeg)` and `docker image rm vidcut-ffmpeg:lts vidcut-ffmpeg:release vidcut-ffmpeg:master`.
- A newer release: put its version and tarball sha256 in `scripts/ffmpeg/release.env` (the steps are there).

**The macOS build, checked from Windows:** `cargo clippy --target aarch64-apple-darwin --workspace --all-targets -- -D warnings`, after `rustup target add aarch64-apple-darwin`. Nothing is linked or run. With `FFMPEG_DIR` set as usual, it needs macOS's C headers from Zig (`winget install zig.zig`): set `BINDGEN_EXTRA_CLANG_ARGS_aarch64_apple_darwin="-isystem ZIG/lib/include -isystem ZIG/lib/libc/include/any-macos-any"`, ZIG being the folder with `zig.exe`.

**A smaller FFmpeg:** `VIDCUT_FFMPEG_WITHOUT=zscale,libx264` treats those parts as missing.

**Media inspection** (`crates/vidcut-av/examples/`):
- `read_barcode FILE...`: the barcode index in images or video.
- `packets FILE [FROM TO]`: video packets in decode order, with pts, dts and keyframe flags.
- `filmstrip FILE OUTDIR [N]`: pool thumbnails, timed.
- `tiles FILE [HEIGHT] [gpu]`: timeline tiles, timed at spacings from 0.25 to 30 s; `gpu` decodes on the GPU in use.
- `seek_check FILE...`: seeks to every frame of barcode fixtures on the CPU and on each GPU decoder, and lists the seeks that show other frames.
- `player_load PROJECT [SECONDS] [gpu]` (release build): plays the timeline with the real player and a paced device, presenting 60 times a second; alone, beside four filmstrip workers and a job that yield to players at the lowest priority, and the load alone. It prints the presents that showed the picture before, the tiles and job frames a second, the threads at most and the peak memory.
- `decode_speed FILE [FRAMES]`: decoding speed, cores and seek times on the CPU and on each GPU decoder.
- `passes FILE START_S SECONDS CODEC ENCODER MBPS [key=value…]` (release build): exports a stretch at a bitrate (`qN`: constant quality) with `video.*` settings (`passes=2`), and prints the bitrate reached, the time and the luma PSNR.

**Debugging:**
- `VIDCUT_DEBUG=1` prints to stderr what vidcut decodes and draws, and why. A GUI still ending 10 s after its window closed says the stage and every thread's wait state, then aborts under `VIDCUT_DEBUG` (exits with 1 otherwise): `VIDCUT_DEBUG=1 gdb -q -batch -ex run -ex "thread apply all bt 15" --args target/debug/vidcut -p PROJECT` shows every thread's stack at the hang.
- `VIDCUT_AV_LOG=error` (or `warning`, `verbose`, `debug`) shows FFmpeg's own messages.
- `VIDCUT_HWDEC` picks the player's GPU decoder: `nvdec[:CUDA index]`, `vaapi[:render node]`, `d3d11va[:adapter index]`, `videotoolbox` or `none`.
- `WGPU_BACKEND` (`vulkan`, `metal`, `dx12`) and `WGPU_POWER_PREF` (`low`, `high`) pick wgpu's backend and adapter. `WGPU_VALIDATION=0` turns off the validation layers in debug builds.
