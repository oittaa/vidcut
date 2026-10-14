# Developer tools

**Fixtures:** generate one (cached, prints its path):

```bash
cargo run --release -p vidcut-av --example make_fixture -- hevc10 mkv 3840x2160 60 600
```

`g=GOP` and `b=BFRAMES` set the keyframe spacing and B-frames (H.264 with B-frames has open GOPs). `r=90` (180, 270) stores the pictures turned, like phone video.

**GPU encoders:** `cargo run -p vidcut-av --example encoders` test-encodes every GPU encoder (NVENC, VAAPI, AMF, QSV) on every GPU, uncached.

**Other FFmpeg versions:** `scripts/ffmpeg-check.sh [TARGET…]` (Docker) runs `scripts/check.sh full` against the checkout in a container per target, and prints a summary.
- Targets: `lts` (Ubuntu 26.04's FFmpeg 8.0), `release` (the newest release) and `master` (FFmpeg's git master, allowed to fail). Default: `lts release`.
- `--fast` runs `scripts/check.sh`; `-- CMD` runs any command, `-- bash` a shell.
- The containers have no GPU, display or audio device.
- FFmpeg's build (`scripts/ffmpeg/build-ffmpeg.sh`) is a cached image layer. `VIDCUT_FFMPEG_MASTER=COMMIT` pins master.
- Each target's build takes about 5.5 GB, the images about 3 GB. To free them: `docker volume rm $(docker volume ls -q -f name=vidcut-ffmpeg)` and `docker image rm vidcut-ffmpeg:lts vidcut-ffmpeg:release vidcut-ffmpeg:master`.
- A newer release: put its version and tarball sha256 in `scripts/ffmpeg/release.env` (the steps are there).

**The macOS build, checked from Windows:** `cargo clippy --target aarch64-apple-darwin --workspace --all-targets -- -D warnings`, after `rustup target add aarch64-apple-darwin`. Nothing is linked or run. With `FFMPEG_DIR` set as usual, it needs macOS's C headers from Zig (`winget install zig.zig`): set `BINDGEN_EXTRA_CLANG_ARGS_aarch64_apple_darwin="-isystem ZIG/lib/include -isystem ZIG/lib/libc/include/any-macos-any"`, ZIG being the folder with `zig.exe`.

**A smaller FFmpeg:** `VIDCUT_FFMPEG_WITHOUT=zscale,libx264` treats those parts as missing.

**Media inspection** (`crates/vidcut-av/examples/`):
- `read_barcode FILE...`: the barcode index in images or video.
- `packets FILE [FROM TO]`: video packets in decode order, with pts, dts and keyframe flags.
- `filmstrip FILE OUTDIR [N [WxH]]`: pool thumbnails, timed, with the frames copied down from the GPU (`WxH`: a box other than 192×108).
- `tiles FILE [HEIGHT] [gpu]`: timeline tiles, timed at spacings from 0.25 to 30 s; `gpu` decodes on the GPU in use.
- `seek_check FILE...`: seeks to every frame of barcode fixtures on the CPU and on each GPU decoder, and lists the seeks that show other frames.
- `proxy_check [--side N] FILE...` (release build): makes a proxy of each barcode fixture with every proxy encoder here, decoding on the CPU and on each GPU decoder. For each it prints how the frames went, the frames copied down and up, and the frames whose barcode or timestamp isn't the source's.
- `decode_speed FILE [FRAMES]`: decoding speed, cores and seek times on the CPU and on each GPU decoder.
- `passes FILE START_S SECONDS CODEC ENCODER MBPS [key=value…]` (release build): exports a stretch at a bitrate (`qN`: constant quality) with `video.*` settings (`passes=2`), and prints the bitrate reached, the time and the luma PSNR.

**Debugging:**
- `VIDCUT_DEBUG=1` prints to stderr what vidcut decodes and draws, and why.
- `VIDCUT_AV_LOG=error` (or `warning`, `verbose`, `debug`) shows FFmpeg's own messages.
- `VIDCUT_HWDEC` picks the player's GPU decoder: `nvdec[:CUDA index]`, `vaapi[:render node]`, `d3d11va[:adapter index]`, `videotoolbox` or `none`.
- `WGPU_BACKEND` (`vulkan`, `metal`, `dx12`) and `WGPU_POWER_PREF` (`low`, `high`) pick wgpu's backend and adapter. `WGPU_VALIDATION=0` turns off the validation layers in debug builds.
