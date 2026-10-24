# Developer tools

**Fixtures:** this command makes a fixture, caches it and prints its path:

```bash
cargo run --release -p vidcut-av --example make_fixture -- hevc10 mkv 3840x2160 60 600
```

- `g=GOP` sets the keyframe spacing. `b=BFRAMES` sets the B-frames.
- H.264 with B-frames has open GOPs.
- `busy` adds noise around the barcode, new in each frame. At 4K60 this gives 260 to 400 Mbit/s, as heavy to decode as camera footage.

**Project speed:** `scripts/history-speed.sh [EDITS]` (release builds).

- It makes a project of 300 media and 2000 clips in a temporary folder, with `FILL` undo steps (100).
- It then times EDITS edits. A command that only reads follows each edit.
- It prints the size of the history after each edit.

**Step-back speed:** `scripts/step-back-speed.py [STEPS] [MB…]` (release builds).

- It opens a window on a 4K 10-bit HEVC clip with keyframes 2 s apart.
- It times STEPS steps back (100), one request for each step.
- It does this with `player.video_memory_mb` at each MB given (1024 8192).
- `CLIP=FILE` times another clip.

**Slow and dropped frames:** `scripts/playback-check.py [--log DIR] [CODEC…]` (`make playback`, release builds).

- It opens a window on 4K60 `busy` AV1, 10-bit HEVC and VP9 clips with keyframes 2 s apart.
- It does each of these moves once: play forward, play backward, jump and play, step both ways, and run and step Dynamic Trim.
- For each move, it prints:
  - the frames that the viewer showed late or skipped,
  - the frames of the two-up that it passed over,
  - the time of each step.
- It fails when a frame was late, skipped or passed over.
- `--log` keeps the debug log (`VIDCUT_DEBUG`) of each window as `DIR/CODEC.log`.

**transcribe.cpp's bindings:** `python3 scripts/transcribe-sys.py BINDINGS`.

- It writes crates/transcribe-cpp-sys/src/transcribe_sys.rs from the bindings that the transcribe-cpp-sys crate ships (its bindings/rust/sys/src/transcribe_sys.rs).
- In the file that it writes, each function calls the library that vidcut loads at run time.
- For a new transcribe.cpp, run it on the bindings of that version. Then put its version and the sizes and SHA-256 of its prebuilt libraries in the catalog (`catalog::TRANSCRIBE_CPP`, `catalog::RUNTIMES`).

**Speech recognition speed:** `python3 scripts/stt-speed.py [--audio FILE] [--seconds S] [--devices KINDS] [MODEL[@PIECE]...]`.

- It times speech recognition models through transcribe.cpp, on each device of the computer.
- It times the load and two runs over the first S seconds (600) of FILE. It also gives the device memory that the model holds.
- The default FILE is `nasa-speech-1979`, which it gets from the media manifest.
- MODEL is a GGUF file, or REPO/FILE of a handy-computer repository on Hugging Face (Parakeet Ultra, Whisper large-v3 and Qwen3-ASR 1.7B at Q8_0).
  - It downloads the models into `$VIDCUT_TEST_DATA/models`.
  - MODEL@30 gives the model 30 s pieces.
- The first run puts two things into `$VIDCUT_TEST_DATA/stt-speed`: the prebuilt transcribe.cpp library for the system, and the Python packages that it needs.
- It checks the clip, the library and the models against their SHA-256.

**Threads:** `scripts/threads.sh [PID] [SECONDS]` (Linux).

- Once a second, it prints the threads of the process by name, with their total and the resident memory.
- Example: `vidcut-play-vid=32` is the decode thread of the player and the 31 threads that FFmpeg started from it.
  - The FFmpeg threads carry the name of the thread that started them.
  - Linux keeps 15 characters of a thread name.
- By default, PID is the newest `vidcut`.
- The `[decoding]` lines of `VIDCUT_DEBUG` give the thread count of each reader when it opens. They also give how many readers are open when one opens or closes.

**GPU encoders:** `cargo run -p vidcut-av --example encoders` makes a test encode with each GPU encoder (NVENC, VAAPI, AMF, QSV) on each GPU. It does not use the cache.

**Other FFmpeg versions:** `scripts/ffmpeg-check.sh [TARGET…]` (Docker) runs `scripts/check.sh full` against the checkout, in a container for each target. Then it prints a summary.

- Targets:
  - `lts`: Ubuntu 26.04's FFmpeg 8.0.
  - `release`: the newest release.
  - `master`: FFmpeg's git master. It is allowed to fail.
  - The default is `lts release`.
- `--fast` runs `scripts/check.sh`. `-- CMD` runs any command, and `-- bash` runs a shell.
- The containers have no GPU, display or audio device.
- `scripts/ffmpeg-host.sh [release|master] [-- CMD]` runs a command on this computer, with its GPU, against the FFmpeg of the target.
  - It copies FFmpeg out of the image to `target/ffmpeg/TARGET/prefix`, once for each image.
  - It sets `FFMPEG_DIR`, `LD_LIBRARY_PATH` and a cargo target directory of its own (`target/ffmpeg/TARGET/cargo`).
  - It sets a linker wrapper that searches the FFmpeg directory first. This is necessary because alsa-sys puts the directory of the system ahead of it.
  - Its FFmpeg runs with the libraries of this computer: Ubuntu 26.04's, as in the image.
- The FFmpeg build (`scripts/ffmpeg/build-ffmpeg.sh`) is a cached image layer. `VIDCUT_FFMPEG_MASTER=COMMIT` pins master.
- The build of each target takes about 5.5 GB, and the images take about 3 GB.
  - To free the space: `docker volume rm $(docker volume ls -q -f name=vidcut-ffmpeg)` and `docker image rm vidcut-ffmpeg:lts vidcut-ffmpeg:release vidcut-ffmpeg:master`.
- For a newer release, put its version and tarball sha256 in `scripts/ffmpeg/release.env`. The steps are in that file.
- Where those builds are already installed, `scripts/with-ffmpeg.sh 8.1 -- cargo test -p vidcut-av --lib` selects one. The other names are `9.0.2` and `lts`.
  - `lts` is Ubuntu 26.04's FFmpeg 8.0, the default that `pkg-config` finds.
  - `8.1` is FFmpeg 8.1.3 at `/opt/ffmpeg/8.1`.
  - `9.0.2` and `release` select the release in `scripts/ffmpeg/release.env`, at `/opt/ffmpeg/9.0.2`.
  - `8.1` and that release get the linker wrapper and a cargo target directory of their own, as with `ffmpeg-host.sh`.

**The macOS build, checked from Windows:** `cargo clippy --target aarch64-apple-darwin --workspace --all-targets -- -D warnings`, after `rustup target add aarch64-apple-darwin`.

- It does not link or run anything.
- With `FFMPEG_DIR` set as usual, it needs the C headers of macOS from Zig (`winget install zig.zig`).
- Set `BINDGEN_EXTRA_CLANG_ARGS_aarch64_apple_darwin="-isystem ZIG/lib/include -isystem ZIG/lib/libc/include/any-macos-any"`. ZIG is the folder with `zig.exe`.

**A smaller FFmpeg:** `VIDCUT_FFMPEG_WITHOUT=zscale,libx264` makes vidcut operate as if those parts are missing.

**Media inspection** (`crates/vidcut-av/examples/`):

- `read_barcode FILE...`: the barcode index in images or video.
- `packets FILE [FROM TO]`: video packets in decode order, with pts, dts and keyframe flags.
- `filmstrip FILE OUTDIR [N]`: pool thumbnails, timed.
- `tiles FILE [HEIGHT] [gpu]`: timeline tiles, timed at spacings from 0.25 to 30 s. `gpu` decodes on the GPU in use.
- `seek_check FILE...`: seeks to each frame of barcode fixtures, on the CPU and on each GPU decoder. It lists the seeks that show other frames.
- `player_load PROJECT [SECONDS] [gpu]` (release build):
  - It plays the timeline with the real player and a paced device, which presents 60 times a second.
  - It measures three runs: the player alone, the player beside four filmstrip workers and a job, and that load alone.
  - The workers and the job yield to players, at the lowest priority.
  - One worker at a time decodes beside the player, as timeline tiles do.
  - It prints the presents that showed the previous picture, the tiles and job frames each second, the maximum thread count and the peak memory.
- `decode_speed FILE [FRAMES]`: the decoding speed, the cores and the seek times, on the CPU and on each GPU decoder.
- `passes FILE START_S SECONDS CODEC ENCODER MBPS [key=value…]` (release build):
  - It exports a stretch at a bitrate (`qN`: constant quality), with `video.*` settings (`passes=2`).
  - It prints the bitrate that it reached, the time and the luma PSNR.

**Debugging:**

- `VIDCUT_DEBUG=1` prints to stderr what vidcut decodes and draws, and why.
- A GUI that has not ended 10 s after its window closed tells the stage and the wait state of each thread.
  - Then it aborts under `VIDCUT_DEBUG`. Without `VIDCUT_DEBUG`, it exits with 1.
  - `VIDCUT_DEBUG=1 gdb -q -batch -ex run -ex "thread apply all bt 15" --args target/debug/vidcut -p PROJECT` shows the stack of each thread at the hang.
- `VIDCUT_AV_LOG=error` (or `warning`, `verbose`, `debug`) shows the messages of FFmpeg itself.
- `VIDCUT_HWDEC` selects the GPU decoder of the player: `nvdec[:CUDA index]`, `vaapi[:render node]`, `d3d11va[:adapter index]`, `videotoolbox` or `none`.
- `WGPU_BACKEND` (`vulkan`, `metal`, `dx12`) and `WGPU_POWER_PREF` (`low`, `high`) select the backend and the adapter of wgpu.
- `WGPU_VALIDATION=0` turns off the validation layers in debug builds.
- `VIDCUT_FLASHVSR_DUMP=DIR` makes the AI Best upscaler write its stages as `.npy` files into `DIR`. A check against the official PyTorch code reads them.
  - `inputNNN.npy`: each enlarged input frame, at the model size, 8-bit RGB.
  - `chunkK_noise.npy` and `chunkK_latents.npy`, for each chunk: the noise and the latents after the one step, `[16, 4, H/8, W/8]`.
  - `chunkK_x_in.npy` and `chunkK_x_blockN.npy` (N is 0, 6 and 29), for the first two chunks: the hidden state before block 0 and after block N, `[tokens, 1536]`. The tokens are in the order of the official code.
  - `frameNNN.npy`: each output frame, 8-bit RGB.
