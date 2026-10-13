# Developer tools

**Fixtures:** generate one (cached, prints its path):

```bash
cargo run --release -p vidcut-av --example make_fixture -- hevc10 mkv 3840x2160 60 600
```

**GPU encoders:** `cargo run -p vidcut-av --example encoders` test-encodes every GPU encoder (NVENC, VAAPI, AMF, QSV) on every GPU it runs on, uncached.

**Other FFmpeg versions** (`scripts/ffmpeg-check.sh`, needs Docker): builds an image per target, then runs `cargo build --workspace` and `scripts/check.sh full` in it against the checkout (mounted read-only), and prints a summary.
- Targets: `lts` (Ubuntu 26.04's FFmpeg 8.0), `release` (the newest FFmpeg release, built from the official tarball on Ubuntu 26.04) and `master` (FFmpeg's git master, the same way; an early warning, allowed to fail). Without targets: `lts release`.
- `--fast` runs `scripts/check.sh` instead; `scripts/ffmpeg-check.sh release -- cargo test -p vidcut --test cli` runs any command, `-- bash` a shell.
- The containers have no GPU, display or audio device: the GPU, GUI and audio-device tests stay off (they are features), and the viewer's shader test skips without a wgpu adapter.
- FFmpeg is configured (`scripts/ffmpeg/build-ffmpeg.sh`) with what vidcut uses: x264, x265, SVT-AV1, libvpx, dav1d, Opus, LAME, Vorbis, zimg, libplacebo on Vulkan, VAAPI, NVENC/NVDEC and the CUDA filters (compiled by clang, no CUDA toolkit). Its build is a cached image layer: about a minute and a half on 16 cores, again only when `scripts/ffmpeg/` changes or master moves (`VIDCUT_FFMPEG_MASTER=COMMIT` pins one).
- Caches are Docker volumes: `vidcut-ffmpeg-cargo` (crates), `vidcut-ffmpeg-target-TARGET` and `vidcut-ffmpeg-data-TARGET` (fixtures made with that FFmpeg; the downloaded media are mounted read-only). A target directory takes about 5.5 GB and the images about 3 GB together. On 16 cores a first run of all three targets took 7 minutes, and a repeat run of both default targets takes about a minute. To free the space: `docker volume rm $(docker volume ls -q -f name=vidcut-ffmpeg)` and `docker image rm vidcut-ffmpeg:lts vidcut-ffmpeg:release vidcut-ffmpeg:master`.
- A newer FFmpeg release: put its version and the sha256 of its tarball, checked against its signature, in `scripts/ffmpeg/release.env` (the steps are there), then run `scripts/ffmpeg-check.sh release`.

**FFmpeg logging:** quiet by default (fatal only). Set `VIDCUT_AV_LOG=error` (or `warning`, `debug`) to see FFmpeg's messages when debugging media problems.

**Media inspection** (examples in `crates/vidcut-av/examples/`):
- `read_barcode FILE...`: the barcode index in images or video (compare screenshots with `vidcut frame render`).
- `packets FILE [FROM TO]`: video packets in decode order, with pts, dts and keyframe flags (open-GOP questions).
- `filmstrip FILE OUTDIR [N]`: pool thumbnails, timed.
- `tiles FILE [HEIGHT]`: timeline filmstrip tiles, timed at spacings from 0.25 to 30 s.
- `decode_speed FILE [FRAMES]`: frames a second (and the cores they took) and seek times decoding on the CPU and on each GPU decoder (NVDEC; VA-API on each render node; Direct3D 11 on each adapter), the GPUs' frames left there and copied down.
- `passes FILE START_S SECONDS CODEC ENCODER MBPS [key=value…]` (release build): exports a stretch at a bitrate (`qN`: constant quality) with `video.*` settings such as `passes=2 first_pass=full` or `multipass=full lookahead=0`, and prints the bitrate reached, the time and the luma PSNR (spec D45).

**Preview debugging:**
- `VIDCUT_DEBUG_PLAYER=1` traces to stderr what the player engines decode, the session's presents, what the viewer thread draws, key presses and timeline gestures.
- `tests/gui.rs` sets it for its GUI. A failed run keeps its temp dir, which the failure names: the screenshots and the GUI's stderr.
- `VIDCUT_HWDEC` picks the GPU decoder the player uses instead of the best one: `nvdec`, `vaapi` (`vaapi:/dev/dri/renderD129` for a render node), `d3d11va` (`d3d11va:1` for an adapter by its index) or `none`.
- `WGPU_BACKEND` (`vulkan`, `metal`, `dx12`) and `WGPU_POWER_PREF` (`low`, `high`) pick wgpu's backend and adapter (eframe reads them); `WGPU_VALIDATION=0` turns the validation layers off in debug builds.
