# AGENTS.md

Guide for AI agents (and humans) working on this repository.

vidcut is a video cutting tool modeled on DaVinci Resolve's Cut page, in Rust: the FFmpeg libraries in-process (`ffmpeg-next`), its own player engine, an egui GUI on wgpu. [README.md](README.md) says what it does and how to build and use it.

## Ground rules

1. **The Cut page is the spec.** Default behavior is whatever Resolve's Cut page does; see [docs/cut-page-spec.md](docs/cut-page-spec.md) (with manual page numbers).
   - Deviate only when you have a clearly better idea, and record it in [docs/deviations.md](docs/deviations.md) with Resolve's behavior and page, our behavior, and the justification.
2. **The code never names DaVinci Resolve.** Nothing in `crates/`, comments included, mentions Resolve or cites its manual's pages, and neither does anything vidcut shows or prints (GUI text, `--help`, the schema, MCP text, errors). Cite the spec instead (`spec §7`, `decision D41`, `deviation X9`); the spec maps to the manual. The only exception is file-format data other editors need, such as the OTIO metadata keys they read.
3. **Everything is scriptable:** every GUI action can also be done from the CLI, IPC and MCP.
4. **No media in git.**
   - Test media, generated fixtures and the manual live in `$VIDCUT_TEST_DATA` (default `~/Downloads/vidcut-tmp`), the one place for test data. Test outputs go to temp dirs.
   - `scripts/check.sh` (the pre-commit hook and CI) refuses any binary file, whatever its name, so `.gitignore` lists no file types.
   - Real-world clips are listed in `tests/media-manifest.toml` (URL, license, sha256) and fetched by `scripts/fetch-test-media.sh`.
5. **No ffmpeg/ffprobe subprocesses.** Use `vidcut-av`, the FFmpeg libraries, for exports and the preview alike.
6. **License:** MIT.
7. **FFmpeg:** 8.0 (the latest Ubuntu LTS) up to the newest release. No code for FFmpeg before 8.0.

## Quality gates

These are mandatory.

| When | Command | Runs |
|---|---|---|
| Before **every commit** (enforced by `.githooks/pre-commit`) | `scripts/check.sh` | no binary files in git, `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, unit tests (`--lib --bins`) |
| After a **bigger change** | `scripts/check.sh full` | the above plus all integration/e2e tests and doc tests |

**Enable the hook once per clone:** `git config core.hooksPath .githooks`.

**CI** (`.github/workflows/check.yml`) runs only the fast gate, on pull requests and pushes to master. `.github/workflows/ffmpeg.yml` builds against the newest FFmpeg release and master weekly and on demand.

## Docs

- [README.md](README.md): features, requirements, build, CLI basics. Keys: [docs/cut-page-spec.md](docs/cut-page-spec.md) §11.
- [docs/cli.md](docs/cli.md): every command, JSON output and exit codes, live control of a running GUI.
- [docs/cut-page-spec.md](docs/cut-page-spec.md): the behavioral spec; [docs/deviations.md](docs/deviations.md): where vidcut differs.
- [docs/decisions/](docs/decisions/): architecture records (preview, export, GUI, agents, player).
- [docs/layout.md](docs/layout.md): crates and modules; [docs/tools.md](docs/tools.md): debug and test tools; [docs/gotchas.md](docs/gotchas.md): findings not in the records.
- [docs/roadmap.md](docs/roadmap.md): what comes next; [docs/todo/](docs/todo/): open work, one file per item.
