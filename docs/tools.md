# Developer tools

**Test media:** `cargo run --release -p vidcut-av --example make_fixture -- hevc10 mkv 3840x2160 60 600` generates a fixture (cached; prints its path). `scripts/fetch-test-media.sh` fetches the real-world clips in `tests/media-manifest.toml`.

**OTIO checks:** `tests/cli.rs` checks vidcut's files against OpenTimelineIO's Python library when it is installed (`pip install opentimelineio`; `VIDCUT_OTIO_PYTHON` names a Python that has it); CI installs the version in `tests/otio/requirements.txt`.

**Other FFmpeg versions:** `scripts/ffmpeg-check.sh [lts|release|master…]` (Docker) builds FFmpeg in an image per target and runs `scripts/check.sh full` there against the checkout, read-only. Default targets: `lts release`.
- `lts` is Ubuntu 26.04's FFmpeg 8.0, `release` the newest release, `master` FFmpeg's git master (allowed to fail; `VIDCUT_FFMPEG_MASTER=COMMIT` pins one).
- `--fast` runs the fast gate; `-- COMMAND` runs any command (`-- bash` for a shell).
- No GPU, display or audio device in the containers: those tests stay off.
- A newer release: put its version and checked tarball sha256 in `scripts/ffmpeg/release.env` (steps there), then run `scripts/ffmpeg-check.sh release`.
- Cleanup: `docker volume rm $(docker volume ls -q -f name=vidcut-ffmpeg)` and `docker image rm vidcut-ffmpeg:lts vidcut-ffmpeg:release vidcut-ffmpeg:master`.

**macOS build from Windows:** `cargo clippy --target aarch64-apple-darwin --workspace --all-targets -- -D warnings` checks what macOS compiles, without linking. Needs `rustup target add aarch64-apple-darwin`, `winget install zig.zig`, and `BINDGEN_EXTRA_CLANG_ARGS_aarch64_apple_darwin="-isystem ZIG/lib/include -isystem ZIG/lib/libc/include/any-macos-any"` (ZIG: the folder with `zig.exe`).

**Examples** (`cargo run -p vidcut-av --example NAME -- ARGS`):
- `encoders`: test-encode with every GPU encoder on every GPU.
- `read_barcode FILE...`: the barcode index in images or video.
- `packets FILE [FROM TO]`: video packets in decode order, with pts, dts and keyframe flags.
- `filmstrip FILE OUTDIR [N]`, `tiles FILE [HEIGHT] [gpu]`: pool thumbnails and timeline tiles, timed.
- `decode_speed FILE [FRAMES]`: decoding speed and seek times on the CPU and each GPU decoder.
- `passes FILE START_S SECONDS CODEC ENCODER MBPS [key=value…]` (release build): bitrate, time and PSNR of an export stretch with given `video.*` settings.

**Environment:**
- `VIDCUT_AV_LOG=error|warning|debug`: FFmpeg's messages (default: fatal only).
- `VIDCUT_DEBUG_PLAYER=1`: trace the player, presents, viewer drawing, keys and gestures to stderr. A failed `tests/gui.rs` run keeps its temp dir with screenshots and the GUI's stderr.
- `VIDCUT_HWDEC=nvdec|vaapi[:/dev/dri/renderD129]|d3d11va[:1]|none`: the player's GPU decoder.
- `WGPU_BACKEND=vulkan|metal|dx12`, `WGPU_POWER_PREF=low|high`, `WGPU_VALIDATION=0`: wgpu's backend, adapter and validation layers.
