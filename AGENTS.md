# AGENTS.md

Guide for AI agents (and humans) working on this repository. Keep it current: when behavior, commands or conventions change, update this file in the same change.

## What vidcut is

A relatively simple video cutting tool modeled on DaVinci Resolve's **Cut page**:
- media pool, source/timeline viewer, dual timeline, ripple editing
- export with presets, GPU encoding, audio-only, range and per-clip export
- no effects, transitions, titles or color

It is written in Rust:
- **GUI:** egui/eframe with the glow (OpenGL) backend.
- **Media:** the FFmpeg libraries in-process via `ffmpeg-next`. Never shell out to the `ffmpeg` CLI.
- **Preview:** libmpv, loaded at runtime.

**Everything must be scriptable:** each GUI action is a serializable Request, also reachable from the CLI and from a JSON-RPC socket on the running GUI.

## Ground rules

1. **The Cut page is the spec.** Default behavior is whatever Resolve's Cut page does; see [docs/cut-page-spec.md](docs/cut-page-spec.md) (with manual page numbers).
   - Deviate only when you have a clearly better idea, and record it in [docs/deviations.md](docs/deviations.md) with Resolve's behavior and page, our behavior, and the justification.
   - When the manual is silent or contradictory, add a decision to §12 of the spec.
   - The manual is `DaVinci_Resolve_21.1_Reference_Manual.pdf` in `$VIDCUT_TEST_DATA`. Read pages with `pdftotext -f N -l N <pdf> -`; the Cut page is pp. 635–790. Never paste manual text into the repo; describe behavior in your own words.
2. **One mutation path.**
   - Every feature is a `Request` handled by the core Session, so the GUI, CLI, IPC and (later) MCP have identical capabilities.
   - GUI gestures emit one Request when they complete.
   - Never mutate project state from GUI code directly.
3. **No media in git.**
   - Test media, generated fixtures and the manual live in `$VIDCUT_TEST_DATA` (default `~/Downloads/vidcut-tmp`).
   - Real-world clips are listed in `tests/media-manifest.toml` (URL, license, sha256) and fetched by `scripts/fetch-test-media.sh`.
4. **No ffmpeg/ffprobe subprocesses.** Use `vidcut-av`, the FFmpeg libraries. libmpv is used only for interactive preview.
5. **License:** MIT. Don't add dependencies with licenses incompatible with MIT distribution of our source.

## Workspace layout

| Crate | Purpose | Depends on |
|---|---|---|
| `crates/vidcut-core` | Project model, time and timecode math, edit operations, Request/Response protocol, Editor (undo/rev), project store | — (pure Rust; fast to build and test) |
| `crates/vidcut-av` | FFmpeg layer: probe, exact-seek decode, encode/mux, export engine, thumbnails, waveforms, still frames, encoder detection, test-fixture generator | ffmpeg-next |
| `crates/vidcut-mpv` | libmpv via `dlopen` (`libloading`): client API and render API (OpenGL and software) | libmpv.so.2 at runtime only |
| `crates/vidcut` | The `vidcut` binary: Session/host, IPC, CLI (clap), GUI (eframe), later MCP | all of the above |

Docs:
- `docs/cut-page-spec.md`: the behavioral spec.
- `docs/deviations.md`: intentional differences from Resolve.
- `docs/decisions/`: short architecture decision records.

## Build and run

```bash
cargo build --workspace
cargo run -p vidcut            # GUI (no subcommand)
cargo run -p vidcut -- --help  # CLI
```

**System packages** (Ubuntu 26.04):

```bash
sudo apt install libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libswscale-dev libswresample-dev libavdevice-dev libclang-dev libmpv2 rustfmt rust-clippy
```

**Toolchain notes:**
- The distro Rust is 1.93; the workspace declares `rust-version = "1.93"`, so the resolver picks compatible crates. That's why we're on eframe **0.35** (0.36 needs Rust 1.95).
- ffmpeg-next resolves to 8.1.x and builds fine against the system FFmpeg 8.0 headers.
- Dependencies build with `opt-level = 2` even in dev, while our crates stay unoptimized.

## Quality gates

These are mandatory.

| When | Command | Runs |
|---|---|---|
| Before **every commit** (enforced by `.githooks/pre-commit`) | `scripts/check.sh` | `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, unit tests (`--lib --bins`) |
| After a **bigger change**, and before a milestone is called done | `scripts/check.sh full` | the above plus all integration/e2e tests and doc tests |
| Also, when encoders, hwdec or real-media paths change | `scripts/check.sh full --features vidcut-av/gpu-tests` | GPU encode tests (NVENC/VAAPI) |

**Enable the hook once per clone:** `git config core.hooksPath .githooks`.

**A bigger change** is anything touching:
- time mapping, decode/seek, the export engine
- EDL/preview, the Request/IPC protocol
- the project file format or store, or edit-operation semantics
- more than one crate

**Rules:**
- **Never commit with failing checks.** Don't skip, ignore or delete tests, or add `#[allow(clippy::...)]`, to get green, unless there's a comment justifying it.
- **Unit tests** (`#[cfg(test)]` in `src/`) must not need media files, a GPU, a display or the network, so the fast gate stays fast and deterministic.
- **Integration tests** (`tests/`) may generate fixtures or use downloaded media. They skip with a clear message when something isn't available.

## Testing conventions

- **Fixtures:** generated by `vidcut-av` into `$VIDCUT_TEST_DATA/fixtures/` and cached by spec hash.
  - The video is a **barcode**: the frame index encoded as luma blocks, which survives lossy codecs, so frame-exactness can be asserted after any decode or encode.
  - The audio carries a 1 kHz beep at the start of every second, for measuring A/V offset.
- **Edit-operation tests** cite the spec section or manual page they check.
- **Golden tests** (insta snapshots in `src/snapshots/`) cover protocol output that agents parse.
  - After an intended change, regenerate with `INSTA_UPDATE=always cargo test`.
  - Review the diff before committing.
- **Property tests** (proptest) run random edit sequences.
  - Failing seeds land in `proptest-regressions/`. Commit them, so they replay forever.
  - For a deeper run: `PROPTEST_CASES=5000 cargo test -p vidcut-core prop_tests`.
- **Test outputs** go to temp dirs (`tempfile`), never into the repo.

## Driving vidcut as an agent

*Status: the headless CLI edits (M1) and exports (M2). Live control of a running GUI arrives in M3, MCP in M7.*

**Discovering commands:**
- Every CLI subcommand is one JSON-RPC method on one `Request`.
- `vidcut schema` prints the whole protocol as JSON Schema; `vidcut schema edit.trim` prints one method's params.
- `vidcut --help` and `vidcut <cmd> --help` show examples.

**Basic use:**

```bash
vidcut new talk.vidcut                      # format comes from the first video added
export VIDCUT_PROJECT=talk.vidcut           # or pass -p FILE every time
vidcut media add cam.mp4 screen.mkv --json  # -> media ids m1, m2
vidcut edit append m1 --range 10s..2m
vidcut edit split --at 1m,1m30s
vidcut edit delete V1@1m10s                 # the clip under 1m10s; ripples Track 1
vidcut timeline --json                      # clips and gaps with frames and timecodes
vidcut undo                                 # works across separate runs
```

**Times:**
- In strings, every time needs a unit: `450f`, `15s`, `1m30s`, `250ms`, `00:01:00:00` (non-drop-frame timecode), `HH:MM:SS.mmm`, `start`, `end`, `in`, `out`.
- In JSON params, a bare integer is a frame number.
- Ranges are half-open: `A..B` excludes B; `A..+D` is D long.

**Clips:** refer to them by id (`c17`) or by position (`V1@<time>`).

**JSON output:**
- `--json` prints `{"rev", "changes": {"created", "modified", "deleted"}, "result", "via"}`.
- Errors print `{"error": {"code", "message"}}` and exit with the matching code:

| Code | Error |
|---|---|
| 0 | ok |
| 1 | `INTERNAL` |
| 2 | `USAGE` |
| 3 | `NOT_FOUND` |
| 4 | `INVALID_EDIT` |
| 5 | `CONFLICT` |
| 6 | `NO_INSTANCE` |
| 7 | `MEDIA` |
| 8 | `EXPORT` |
| 9 | `CANCELLED` |
| 10 | `DEPENDENCY` |

**Safe automation:**
- `--dry-run` reports `changes` without writing anything.
- `--if-rev N` fails with `CONFLICT` if someone else changed the project after you read revision `N`.
- `vidcut apply edits.jsonl` (or `-` for stdin) applies many requests atomically, as one undo step. The format is one `{"method", "params"}` object per line.
- Parallel `vidcut` processes on one project are serialized by a lock and never corrupt it.

**Raw requests:** `vidcut rpc edit.split '{"at": ["1m", 2700]}'` sends one exactly as written.

**Export and stills:**
- `vidcut export out.mp4` renders the timeline: its In–Out range when both marks are set, else everything.
  - The extension picks the container, and also the preset unless `--preset` is given.
  - Extra options: `--preset youtube-1080p`, `--encoder nvenc|vaapi|cpu|auto`, `--set video.quality=20`, `--range 1m..2m`, `--per-clip`, `--audio-only`, `--overwrite`.
  - With `--dry-run` it prints the plan and the chosen encoder without encoding.
  - Ctrl-C cancels cleanly (exit 9, no partial file).
- `vidcut presets` lists the presets; `vidcut presets NAME` shows one's settings.
- `vidcut encoders` shows which GPU encoders work here (cached; `--refresh` re-tests).
- `vidcut frame --at 1m -o f.png [--width 640]` renders what the timeline shows at a frame. `--media m1` renders a media frame instead. Use it to look at cuts.

**Still to come:** live GUI control (`seek`, `screenshot`, auto-routing) in M3.

## Tools

**Fixtures:** generate one (cached, prints its path):

```bash
cargo run --release -p vidcut-av --example make_fixture -- hevc10 mkv 3840x2160 60 600
```

**GPU encoders:** `cargo run -p vidcut-av --example encoders` test-encodes every NVENC/VAAPI option, uncached.

**FFmpeg logging:** quiet by default (fatal only). Set `VIDCUT_AV_LOG=error` (or `warning`, `debug`) to see FFmpeg's messages when debugging media problems.

**Preview spikes:**
- `cargo test -p vidcut-mpv --test edl_spike -- --nocapture` runs the mpv EDL/seek semantics experiments headless.
- `cargo run -p vidcut --example mpv_gl_spike -- FILE [FILE2] --auto-exit 8` opens a window with mpv embedded in egui and prints hwdec and drop statistics.

## Gotchas

The preview findings and the time-mapping rules are in [docs/decisions/preview.md](docs/decisions/preview.md); the export engine, sampling rule and encoder decisions are in [docs/decisions/export.md](docs/decisions/export.md). Read them before touching preview, seeking, time conversion or export.

- **libmpv needs `LC_NUMERIC=C`.** `Mpv::new` calls `setlocale` before `mpv_create`.
- **mpv's render thread (the GUI's UI thread) may only call render-thread-safe client functions:**
  - async commands and property calls
  - `mpv_observe_property`, `mpv_wakeup`
  - `mpv_wait_event(…, 0)`

  Synchronous `command`/`get_property` there deadlocks against the video output. The mpv update and wakeup callbacks must not call mpv at all.
- **vsync must stay off** (`GlowConfiguration { vsync: false }`). On GNOME/Wayland with NVIDIA, a vsynced swap blocks forever while the window is hidden.
- **The half-frame bias is applied exactly once.** mpv EDL `start` values are raw source timestamps and carry it, so timeline seeks to frame `j` go to exactly `j/f`.
- **Never send negative absolute seeks.** mpv counts them from the end of the file.
- **MPEG-TS and other index-less containers can't be seeked exactly by mpv.** Preview uses an MKV remux (`vidcut_av::remux`); export reads the original.
- **ffmpeg-next's `Packet::write_interleaved` refuses empty packets.** Encoders such as FLAC emit a final empty packet carrying side data; use `av_interleaved_write_frame` directly.
- **Set packet durations explicitly when muxing** from delayed encoders (x264 with B-frames). Otherwise MP4 edit lists drop the last frame.
- **Set `pkt_timebase` on every decoder** (`codec_ctx.pkt_timebase = stream.time_base`). Without it, FFmpeg can't adjust timestamps when it skips priming samples, and audio drifts by the codec delay.
- **Not handled in export yet:** rotation metadata, interlaced sources, HDR (see docs/decisions/export.md §4).
- **GPUs on the dev machine:**
  - NVIDIA on `renderD128`: NVENC works, VAAPI doesn't.
  - AMD on `renderD129`: VAAPI H.264/HEVC.
  - The GL context is on NVIDIA, so preview hwdec is nvdec with zero-copy CUDA frames.
