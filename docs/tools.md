# Developer tools

**The build:** vidcut gives its version with its build, as `0.1.0-g7c1ba35a-20261110`.

- The build is the commit and the date of the commit. `-dirty` at the end tells that the build had changes that no commit holds.
- `vidcut --version`, the status bar, `vidcut doctor`, the report of an export and the MCP server give it.
- `crates/vidcut/build.rs` reads the commit from git (`VIDCUT_BUILD`). A build without git gives the version alone.

**Fixtures:** this command makes a fixture, caches it and prints its path:

```bash
cargo run --release -p vidcut-av --example make_fixture -- hevc10 mkv 3840x2160 60 600
```

- `g=GOP` sets the keyframe spacing. `b=BFRAMES` sets the B-frames.
- H.264 with B-frames has open GOPs.
- `busy` adds new noise around the barcode in each frame. Thus the stream is heavy to decode.

**GPU check:** `scripts/gpu-check.sh` (release build) shows what runs on the GPU in four common exports and in the viewer.

- It uses 5 s of the Big Buck Bunny 4K60 clip from 1:00, plain and at 150% zoom.
- For each, it exports with the hardware encoder and prints the export's report, the time and the CPU time.
- It prints the PSNR of 12 frames against the CPU encoder's frames.
- It prints where the viewer's picture was held, and its PSNR against the export pipeline's frame.
- `scripts/fetch-test-media.sh bbb-2160p60` gets the clip.

**Project speed:** `scripts/history-speed.sh [EDITS]` (release builds).

- It makes a project of 300 media and 2000 clips in a temporary folder, with `FILL` undo steps (100).
- It then times EDITS edits. A command that only reads follows each edit.
- It prints the size of the history after each edit.

**Step-back speed:** `scripts/step-back-speed.py [STEPS] [MB…]` (release builds).

- It opens a window on a 4K 10-bit HEVC clip with keyframes 2 s apart.
- It times STEPS steps back (100), one request for each step.
- It does this with `player.video_memory_mb` at each MB given (1024 8192).
- `CLIP=FILE` times another clip.

**Face speed:** `python3 scripts/face-speed.py [SIZE…]` (release build).

- It runs `motion.find` with the slow face model on 8 s of the clip `tos-720p-mov`, at each SIZE given (640 1280).
- It prints the time in the model for each picture.
- From the `[subjects] model call` lines, it prints the faces found for each picture. A wrong model output can give many faces in each picture.
- From the same lines, it prints the time of each step for each picture. It gives the first call apart, because only the first call has the clock step.
- It compares the wait in the early calls with the wait in the last calls. A longer wait at the end can show a GPU that slows when it gets hot.
- From the `[subjects] gpu` lines, it prints the GPU time for each picture, each stage and the slowest layers.
- The finds and their cache go to a temporary folder. Thus each run measures again.

**Stabilize speed:** `python3 scripts/stabilize-speed.py [--modes M,M] [--runs N] [--seconds S] [--json FILE] CLIP…` (release build).

- For each clip and each mode, it runs `picture.stabilize` on the whole clip.
- It prints the frames a second of the analysis, the zoom, and the `motion` and the `shake` before and after.
- It also prints the cost of the analysis, from `scripts/usage.py`.
- Before each timed run, it removes the analysis from its cache. Thus each run reads and measures every frame again.
- An untimed run of each clip comes first. Thus the decoder and the disk are ready.
- The time includes the start of vidcut and the decode, with the GPU settings of this computer.
- `--runs N` prints the median of N runs. `--seconds S` uses only the first S seconds of each clip. `--json FILE` writes each result.
- The projects and their cache go to a temporary folder.

**Cost of a program:** `python3 scripts/usage.py -- COMMAND…`

- It runs the command and prints the wall time, the CPU time, the CPU use, the peak memory and the use of each GPU engine.
- CPU use is the CPU time over the wall time, in cores and in percent of all threads.
- GPU use is the mean busy time of each engine type (3D, Compute, VideoDecode), as Task Manager shows it.
- `--name NAME --log FILE` samples the processes with that name four times a second, until it stops. Use it for a program that runs already, for example an other editor. `--pid PID` samples one process.
- `--log FILE --from T0 --to T1` prints the cost between two times of that log, in seconds since 1970.
- Memory, GPU use and `--name` need Windows.

**Stabilize scores:** `uv run --with opencv-python-headless --with numpy scripts/stabilize-score.py --original VIDEO [--clips N] [--names A,B] NAME=VIDEO…`

- It gives the three usual scores of video stabilization (Liu et al. 2013) for each stabilized video against the original.
- Cropping: the mean share of the picture that the output keeps. Distortion: the worst stretch of a frame. Stability: the share of the slow motion in the camera path.
- Each score goes from 0 to 1. Higher is better.
- `--clips N` scores each N frames as one clip.
- The docstring of the script gives each formula.

**Face check:** `uv run scripts/face-check.py [SIZE]` (release build).

- It finds the first step of the slow face model where the GPU result is wrong.
- It runs `motion.find` with `VIDCUT_SUBJECTS_DUMP` on 1 s of the clip `tos-720p-mov`, at SIZE (640 when not given).
- It gives the same pictures to onnxruntime on the CPU, and reads the output of each step from the ONNX graph.
- For each step, it prints the largest difference as a share of the largest value. A share of more than 0.01 is far off.
- It needs what the face speed script needs, and numpy, onnx and onnxruntime. `uv run` gets these packages, and a Python version that onnxruntime supports.
- `RANGE=A..B` checks another part of the clip. `VIDCUT_GPU_CONV` selects the convolution kernels.

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

**Speech recognition speed:** `python3 scripts/stt-speed.py [--audio FILE] [--seconds S] [--devices KINDS] [--json FILE] [MODEL[@PIECE]...]`.

- It times speech recognition models through transcribe.cpp, on each device of the computer.
- It times the load and two runs over the first S seconds (600) of FILE. It also gives the device memory that the model holds.
- The default FILE is `nasa-speech-1979`, which it gets from the media manifest.
- MODEL is a GGUF file, or REPO/FILE of a handy-computer repository on Hugging Face (Parakeet Ultra, Whisper large-v3 and Qwen3-ASR 1.7B at Q8_0).
  - It downloads the models into `$VIDCUT_TEST_DATA/models`.
  - MODEL@30 gives the model 30 s pieces.
- The first run puts two things into `$VIDCUT_TEST_DATA/stt-speed`: the prebuilt transcribe.cpp library for the system, and the Python packages that it needs.
- It checks the clip, the library and the models against their SHA-256.
- `--json FILE` also writes the results to FILE.

**Threads:** `scripts/threads.sh [PID] [SECONDS]` (Linux).

- Once a second, it prints the threads of the process by name, with their total and the resident memory.
- Example: `vidcut-play-vid=32` is the decode thread of the player and the 31 threads that FFmpeg started from it.
  - The FFmpeg threads carry the name of the thread that started them.
  - Linux keeps 15 characters of a thread name.
- By default, PID is the newest `vidcut`.
- The `[decoding]` lines of `VIDCUT_DEBUG` give the thread count of each reader when it opens. They also give how many readers are open when one opens or closes.
- The `[subjects]` line of `VIDCUT_DEBUG` is for `motion.find`. It gives the pictures that the model saw, the frames decoded, the time, and the time in the model.
  - With the slow face model, one `[subjects] model call` line comes for each call of the model. It gives the faces found and the time of each step of the call:
    - upload: the pictures go to the GPU.
    - send: vidcut records the passes and sends them to the GPU.
    - clock: the GPU timing of one pass.
    - wait: vidcut waits for the GPU and reads the results back.
    - boxes: vidcut makes the boxes from the results.
  - The GPU works while vidcut uploads the pictures of the next pass. Thus the wait can be shorter than the GPU time.
  - The `[subjects] gpu` lines give the GPU name and the GPU time of one pass, and of each layer in that pass. Each find measures one pass.
  - On a GPU that writes timestamps inside a compute pass, the times come from these timestamps. Under `VIDCUT_DEBUG`, vidcut asks the device for this feature.
  - On another GPU, vidcut runs each dispatch of that pass again on its own and waits for it. The time is the wait on the CPU, with the time to send the dispatch.
  - The time in the model then also holds this second run. The line of the pass names the method.

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
- The builds and the images take GBs of disk. To free the space: `docker volume rm $(docker volume ls -q -f name=vidcut-ffmpeg)` and `docker image rm vidcut-ffmpeg:lts vidcut-ffmpeg:release vidcut-ffmpeg:master`.
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

**A smaller FFmpeg:** `VIDCUT_FFMPEG_WITHOUT=libx264,libopus` makes vidcut operate as if those parts are missing.

**Media inspection** (`crates/vidcut-av/examples/`):

- `read_barcode FILE...`: the barcode index in images or video.
- `packets FILE [FROM TO]`: video packets in decode order, with pts, dts and keyframe flags.
- `tiles FILE [HEIGHT] [gpu]`: timeline tiles, timed at spacings from 0.25 to 30 s. `gpu` decodes on the GPU in use.
- `seek_check FILE...`: seeks to each frame of barcode fixtures, on the CPU and on each GPU decoder. It lists the seeks that show other frames.
- `player_load PROJECT [SECONDS] [gpu]` (release build):
  - It plays the timeline with the real player and a paced device, which presents 60 times a second.
  - It measures three runs: the player alone, the player beside four filmstrip workers and a job, and that load alone.
  - The workers and the job yield to players, at the lowest priority.
  - One worker at a time decodes beside the player, as timeline tiles do.
  - It prints the presents that showed the previous picture, the tiles and job frames each second, the maximum thread count and the peak memory.
- `decode_speed FILE [FRAMES] [START_S]`: the decoding speed, the cores and the seek times, on the CPU and on each GPU decoder.
  - It starts at `START_S` seconds into the file. On Windows, it also prints the GPU memory that the decoding takes.
- `passes FILE START_S SECONDS CODEC ENCODER MBPS [key=value…]` (release build):
  - It exports a stretch at a bitrate (`qN`: constant quality), with `video.*` settings (`passes=2`).
  - It prints the bitrate that it reached, the time and the luma PSNR.

**The AI models and HDR:** `cargo run --release -p vidcut-av --example hdr_models` (`hdr_check`).

- It runs each AI model through two pipes: 16-bit RGB in PQ or HLG, and 16-bit SDR.
- Upscale makes 1920×1080 from 480×270. It compares the result with the real frame, and with Classic.
- Retime Process makes the frame half way between two frames. It compares it with the real frame between them, and with Frame Blend.
  - It leaves out pairs with a shot change.
- It prints these measures of each result against the real frame:
  - `dE`: the mean ΔE ITP (BT.2124) of the pixels.
  - `low`: the mean ΔE ITP of the means of 40×40 blocks, in linear light. It shows the errors of large areas, without the detail.
  - `mean`: the ΔE ITP of the means of the whole frames, in linear light. It shows a shift of all of the picture.
  - `Y`: the mean luminance over the mean luminance of the real frame.
  - `PSNR`: the PSNR of the code values.
- It scores the frames on all cores.
- `hdr_models scale` runs only Upscale, and `hdr_models retime` only Retime Process.
- `hdr_models speeds` measures each model in a process of its own: the time per frame and the GPU memory, on Vulkan and DX12 only.
  - Other work on the GPU makes the times longer. `nvidia-smi` shows it on NVIDIA.
- `hdr_models pictures DIR` writes one frame of each upscaler as 1920×1080 RGB48 files.
- On macOS, it tests the AI of macOS only.
  - The AI of macOS takes the VideoToolbox frames of an export (`surface_picture`). They are P416 in each pipe.
- The clips are in `tests/media-manifest.toml`. `scripts/fetch-test-media.sh NAME` gets each one.

**The viewer's picture:** `vidcut frame render --at TIME --out F.png --viewer` draws a frame of the timeline as the viewer draws it, with no window.

- It uses the player, its GPU decoding, and the viewer's shader on a GPU device of its own (`gui::frame::render`).
- It shows the cues of the shown subtitle track, as the viewer does.
- Without `--viewer`, `frame render` draws the frame with the export's pipeline.
- The answer names the renderer. It gives the layout and the place of each layer of the picture, from the bottom up.
- If the player cannot make the frame, the error is the error of the player.

**The scopes:** `vidcut frame scope waveform --at TIME --out F.png` renders one scope of that same picture, with no window. The others are `parade` and `vectorscope`.

- It draws the frame at the size of the timeline.
- It reads the picture through the Scopes LUT of the project, as the scopes of the GUI do. A file that cannot be read is an error.
- The PNG has the lines of the scale, with no text.
- The answer names `colors`, which is the timeline color space. It gives `marks`, with the label and the place of each line of the scale.

**The refusals of Upscale:** `scripts/upscale-check.sh` asks an export and the viewer about three clips, with the upscaler of macOS. In each case, the two answers must agree.

- It builds vidcut and makes its clips with `make_fixture`. It needs no clicks.
- On macOS, the clip that the scaler enlarges works in both. Elsewhere, both refuse it.
- `VIDCUT` gives another build of vidcut.

**Debugging:**

- `VIDCUT_DEBUG=1` prints to stderr what vidcut decodes and draws, and why.
  - The `[export]` line comes at the end of each export. It tells where the time of the frames went, in seconds.
    - On the export thread: decoding, drawing, mixing the sound, and waiting for the encoder's thread.
    - The drawing has four parts: the decoded picture in, the draw, waiting for a free output buffer, and the encoder's planes.
    - On the encoder's thread: waiting for frames, waiting for the draw, encoding the video and the sound, and the end (flush and trailer).
    - The wait for the draw has two parts: the wait for the GPU, and the copy into the encoder's frame.
    - The draw is the CPU's time to record and submit the GPU's work. It does not wait for the GPU.
    - When NVENC takes the planes in GPU memory, the encoder's thread waits for the GPU. Otherwise, the export thread waits for it in the encoder's planes.
    - The line times only the last pass over the frames. It leaves out the first pass of a two-pass export and the light measurements.
  - The `[upscale]` lines give the time of each step of each AI Fast chunk.
  - The `[flashvsr]` lines give the time of each step of each AI Slow chunk: the input projection, the blocks and the decoder. They also give how many caches are in main memory, how many GB go there and back, and how long AI Slow waits for them.
  - On macOS, an `[ai]` line names the pixel formats that each processor of macOS takes, as four-character codes (`format_in`). A 10-bit or half-float format can carry HDR.
- On macOS, `vidcut doctor` also lists these formats for a 1920×1080 frame rate conversion and a 960×540 super-resolution scale (`macos_frame_processors`).
- A GUI that has not ended 10 s after its window closed (`EXIT_PATIENCE`) tells the stage and the wait state of each thread.
  - Then it aborts under `VIDCUT_DEBUG`. Without `VIDCUT_DEBUG`, it exits with 1.
  - `VIDCUT_DEBUG=1 gdb -q -batch -ex run -ex "thread apply all bt 15" --args target/debug/vidcut -p PROJECT` shows the stack of each thread at the hang.
- `VIDCUT_AV_LOG=error` (or `warning`, `verbose`, `debug`) shows the messages of FFmpeg itself.
- `VIDCUT_HWDEC` selects the GPU decoder of the player: `nvdec[:CUDA index]`, `vaapi[:render node]`, `d3d11va[:adapter index]`, `videotoolbox` or `none`.
- `WGPU_BACKEND` (`vulkan`, `metal`, `dx12`) and `WGPU_POWER_PREF` (`low`, `high`) select the backend and the adapter of wgpu.
- `WGPU_VALIDATION=0` turns off the validation layers in debug builds.
- `VIDCUT_FLASHVSR_DUMP=DIR` makes the AI Slow upscaler write its stages as `.npy` files into `DIR`. A check against the official PyTorch code reads them.
  - `inputNNN.npy`: each enlarged input frame, at the model size, 8-bit RGB.
  - `chunkK_noise.npy` and `chunkK_latents.npy`, for each chunk: the noise and the latents after the one step, `[16, 4, H/8, W/8]`.
  - `chunkK_x_in.npy` and `chunkK_x_blockN.npy` (N is 0, 6 and 29), for the first two chunks: the hidden state before block 0 and after block N, `[tokens, 1536]`. The tokens are in the order of the official code.
  - `frameNNN.npy`: each output frame after the color fix, 8-bit RGB. The official code makes these frames with `--cf`.
- `VIDCUT_INTERPOLATE_DUMP=DIR` makes Retime Process write each frame that it makes as `.npy` files into `DIR`, all 8-bit RGB `[H, W, 3]`.
  - `NNNN-a.npy` and `NNNN-b.npy`: the two source frames, at the size of the made frame.
  - `NNNN-out-T-PROCESS.npy`: the made frame, with the weight `T` of the later frame and the process, such as `AIFast`.
  - A check against a PyTorch version of a model reads them.
- `VIDCUT_SUBJECTS_DUMP=DIR` makes `motion.find` write what the model gets and what it finds as `.npy` files into `DIR`.
  - `NNNNN.npy`: each picture at the size of the model, as blue, green and red planes of 8 bits, `[3, H, W]`.
  - `boxes.npy`: one row for each found box, `[K, 6]`: the picture, the center, the size and the score. The center and the size are fractions of the picture content.
  - `content.npy`: the width and the height of the picture content, in pixels of the model input.
  - With the slow face model, `stage-NN-STEP.npy`: the output of each step of the network for the first two pictures, `[N, H, W, C]`.
  - vidcut runs the pass again for each step and stops it after that step (`Gpu::stop_after`). Thus later steps cannot write over the output.
  - A check against an ONNX runtime reads them.
- `VIDCUT_GPU_CONV` turns off the fast convolution kernels of the GPU layers (`compute::ops`). A different result then shows a fault in a kernel that is off.
  - `direct`: no Winograd convolutions (`WINO_GEMM`).
  - `plain`: no Winograd convolutions and no `CONV4`. Each convolution uses `CONV`.
- `vidcut models list` prints the catalog of models and speech services. `--json` gives all of it.
- `vidcut models check MODEL` checks the SHA-256 of each file of a model, then loads it.
  - For a transcribe.cpp model, it compares what the model reports with the catalog.
  - It compares the languages, the kind of times, the longest audio and the most speakers.
  - A difference is an error that names the field.
  - `--download` gets the files that this computer doesn't have first.
- To change a model or a service in the catalog (`crates/vidcut-core/src/catalog.rs`):
  1. Change only the entry of the model.
  2. Take the sizes and the SHA-256 from the release data of GitHub or Hugging Face.
  3. Take each limit from the documentation of its maker, and set the date of the check.
     - A limit that transcribe.cpp reads from the model file has `Source::ModelFile`.
     - Without a source, the limit is not recorded (`Source::Unrecorded`).
  4. Run `vidcut models check MODEL --download`, then the commit gate.
  5. The pull request lists each changed field: the old value, the new value and the source.
