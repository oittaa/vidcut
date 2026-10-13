# Developer tools

## Fixtures and test media

- Generate a fixture (cached; prints its path): `cargo run --release -p vidcut-av --example make_fixture -- hevc10 mkv 3840x2160 60 600`
- Real-world clips: `scripts/fetch-test-media.sh` (listed in `tests/media-manifest.toml`).

## Other FFmpeg versions

`scripts/ffmpeg-check.sh [TARGET…]` (needs Docker) builds an image per target, runs `cargo build --workspace` and `scripts/check.sh full` in it against the checkout (mounted read-only), and prints a summary.

- **Targets:** `lts` (Ubuntu 26.04's FFmpeg 8.0), `release` (the newest FFmpeg release, built from the official tarball) and `master` (FFmpeg's git master; allowed to fail). Default: `lts release`.
- `--fast` runs `scripts/check.sh` instead. `scripts/ffmpeg-check.sh release -- cargo test -p vidcut --test cli` runs any command; `-- bash` opens a shell.
- The containers have no GPU, display or audio device: the GPU, GUI and audio-device tests stay off, and the viewer's shader test skips without a wgpu adapter.
- FFmpeg is built (`scripts/ffmpeg/build-ffmpeg.sh`) with what vidcut uses: x264, x265, SVT-AV1, libvpx, dav1d, Opus, LAME, Vorbis, zimg, libplacebo on Vulkan, VAAPI, NVENC/NVDEC and the CUDA filters (compiled by clang, no CUDA toolkit). The build is a cached image layer, rebuilt when `scripts/ffmpeg/` changes or master moves (`VIDCUT_FFMPEG_MASTER=COMMIT` pins a commit).
- Caches are Docker volumes: `vidcut-ffmpeg-cargo` (crates), `vidcut-ffmpeg-target-TARGET` and `vidcut-ffmpeg-data-TARGET` (fixtures made with that FFmpeg; downloaded media are mounted read-only). They take several GB. To free the space:
  ```bash
  docker volume rm $(docker volume ls -q -f name=vidcut-ffmpeg)
  docker image rm vidcut-ffmpeg:lts vidcut-ffmpeg:release vidcut-ffmpeg:master
  ```
- **A newer FFmpeg release:** put its version and its tarball's sha256 (checked against its signature) in `scripts/ffmpeg/release.env` (the steps are there), then run `scripts/ffmpeg-check.sh release`.

## The macOS build, checked from Windows

`cargo clippy --target aarch64-apple-darwin --workspace --all-targets -- -D warnings` checks everything macOS compiles; nothing is linked or run. Setup:

- `rustup target add aarch64-apple-darwin`
- `winget install zig.zig` (macOS's C headers for FFmpeg's bindings, which come from the Windows build's headers via `FFMPEG_DIR`)
- `BINDGEN_EXTRA_CLANG_ARGS_aarch64_apple_darwin="-isystem ZIG/lib/include -isystem ZIG/lib/libc/include/any-macos-any"`, where ZIG is Zig's folder (the one with `zig.exe`)

Running needs a Mac: CI's `macos` job runs `scripts/check.sh`.

## GPU encoders

`cargo run -p vidcut-av --example encoders` test-encodes with every GPU encoder (NVENC, VAAPI, AMF, QSV) on every GPU, uncached.

## Media inspection

Examples in `crates/vidcut-av/examples/`, run with `cargo run -p vidcut-av --example NAME -- ARGS`:
- `read_barcode FILE...`: the barcode index in images or video (compare screenshots with `vidcut frame render`).
- `packets FILE [FROM TO]`: video packets in decode order, with pts, dts and keyframe flags (open-GOP questions).
- `filmstrip FILE OUTDIR [N]`: pool thumbnails, timed.
- `tiles FILE [HEIGHT] [gpu]`: timeline filmstrip tiles, timed at spacings from 0.25 to 30 s; `gpu` decodes on the viewer's GPU decoder (`VIDCUT_HWDEC` picks another).
- `decode_speed FILE [FRAMES]`: frames a second (and cores used) and seek times on the CPU and on each GPU decoder (NVDEC; VA-API on each render node; Direct3D 11 on each adapter), with the GPU's frames kept there and copied down.
- `passes FILE START_S SECONDS CODEC ENCODER MBPS [key=value…]` (release build): exports a stretch at a bitrate (`qN`: constant quality) with `video.*` settings such as `passes=2 first_pass=full` or `multipass=full lookahead=0`, and prints the bitrate reached, the time and the luma PSNR (spec D45).

## Debugging

- `VIDCUT_AV_LOG=error` (or `warning`, `debug`) shows FFmpeg's messages; by default only fatal ones.
- `VIDCUT_DEBUG_PLAYER=1` traces to stderr what the player engines decode, the session's presents, what the viewer thread draws, key presses and timeline gestures. `tests/gui.rs` sets it; a failed run keeps its temp dir (named in the failure) with the screenshots and the GUI's stderr.
- `VIDCUT_HWDEC` overrides the player's GPU decoder: `nvdec`, `vaapi` (`vaapi:/dev/dri/renderD129` for a render node), `d3d11va` (`d3d11va:1` for an adapter by index) or `none`.
- `WGPU_BACKEND` (`vulkan`, `metal`, `dx12`) and `WGPU_POWER_PREF` (`low`, `high`) pick wgpu's backend and adapter; `WGPU_VALIDATION=0` turns off validation layers in debug builds.
