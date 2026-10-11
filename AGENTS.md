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
| Also, when encoders, hwdec or real-media paths change | `scripts/check.sh full --features gpu-tests,media-tests` | GPU encode tests and tests on downloaded clips |

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
- **Golden tests** (insta snapshots) cover protocol and plan output; review snapshot changes deliberately.
- **Test outputs** go to temp dirs (`tempfile`), never into the repo.

## Driving vidcut as an agent

*Status: planned. M0 (scaffold and spikes) is done; the CLI lands in M1, the live socket in M3, MCP in M7. Update this section as each part lands.*

- **Headless:** `vidcut -p project.vidcut <command> --json` edits a project file. Exit codes are stable:
  - 0 ok, 1 internal, 2 usage, 3 not found, 4 invalid edit, 5 conflict
  - 6 no instance, 7 media, 8 export, 9 cancelled, 10 dependency
- **Discover commands:** `vidcut schema [method]` prints JSON Schemas.
- **Batches:** `vidcut -p p.vidcut apply cmds.jsonl` applies them atomically, as one undo step.
- **Look at frames:** `vidcut -p p.vidcut frame --at 00:00:10:00 -o f.png`.
- **Live GUI:**
  - If the GUI has the project open, CLI commands are routed to it automatically.
  - Live-only commands: `vidcut seek …`, `vidcut screenshot -o ui.png`.
  - Raw JSON-RPC: `vidcut rpc <method> '<json>'`.
- **Concurrency:** pass `--if-rev N` to avoid clobbering concurrent edits.

## Tools

**Fixtures:** generate one (cached, prints its path):

```bash
cargo run --release -p vidcut-av --example make_fixture -- hevc10 mkv 3840x2160 60 600
```

**Preview spikes:**
- `cargo test -p vidcut-mpv --test edl_spike -- --nocapture` runs the mpv EDL/seek semantics experiments headless.
- `cargo run -p vidcut --example mpv_gl_spike -- FILE [FILE2] --auto-exit 8` opens a window with mpv embedded in egui and prints hwdec and drop statistics.

## Gotchas

The preview findings and the time-mapping rules are in [docs/decisions/preview.md](docs/decisions/preview.md). Read it before touching preview, seeking or time conversion.

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
- **GPUs on the dev machine:**
  - NVIDIA on `renderD128`: NVENC works, VAAPI doesn't.
  - AMD on `renderD129`: VAAPI H.264/HEVC.
  - The GL context is on NVIDIA, so preview hwdec is nvdec with zero-copy CUDA frames.
