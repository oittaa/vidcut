# AGENTS.md

Guide for AI agents (and humans) working on this repository.

vidcut is a video cutting tool modeled on DaVinci Resolve's Cut page, in Rust. [README.md](README.md) says what it does and how to build it; [docs/design/](docs/design/README.md) how it is built.

## Ground rules

1. **The Cut page is the spec.** Default behavior is whatever Resolve's Cut page does; see [docs/cut-page-spec.md](docs/cut-page-spec.md) (with manual page numbers).
   - Deviate only when you have a clearly better idea, and record it in [docs/deviations.md](docs/deviations.md) with Resolve's behavior and page, our behavior, and the justification.
2. **The code never names DaVinci Resolve, and has no comments.** Nothing in `crates/` mentions Resolve or cites its manual's pages, and neither does anything vidcut shows or prints (GUI text, `--help`, the schema, MCP text, errors). The only exception is file-format data other editors need, such as the OTIO metadata keys they read.
   - No comments in `crates/` except the schema's doc comments (shipped text: `vidcut schema`, `--help`, MCP) and one-line `// SAFETY:` comments. A public unsafe fn also has the `# Safety` section clippy requires, one line under its heading. `scripts/check.sh` enforces it.
   - Why the code is built the way it is goes in the design docs in `docs/design/`.
3. **Everything is scriptable:** every GUI action can also be done from the CLI, IPC and MCP.
4. **No media in git.**
   - Test media, generated fixtures and the manual live in `$VIDCUT_TEST_DATA` (default `~/Downloads/vidcut-tmp`), the one place for test data. Test outputs go to temp dirs.
   - `scripts/check.sh` (the pre-commit hook and CI) refuses any binary file, whatever its name, so `.gitignore` lists no file types.
   - Real-world clips are listed in `tests/media-manifest.toml` (URL, license, sha256) and fetched by `scripts/fetch-test-media.sh`.
5. **No ffmpeg/ffprobe subprocesses.** Use `vidcut-av`, the FFmpeg libraries, for exports and the preview alike.
6. **License:** MIT.
7. **FFmpeg:** 8.0 (the latest Ubuntu LTS) up to the newest release. No code for FFmpeg before 8.0.
8. **A unit test is a unit test:** one behavior, a fixed input, and the output it must give written out, like `assert_eq!(sha("test text"), "abcd")`.
   - It doesn't reimplement the code to work out what to expect, doesn't test a dozen things at once, and isn't 100+ lines stuffed together.
   - Anything that needs more than that is checked by hand: no tests that open a window, play real media or depend on timing.
9. **The GPU first, wherever the user lets it work.** With `gpu.acceleration` and `gpu.decoding` on, decoding and encoding run on the GPU wherever it does them right, in every path, H.264 included.
   - Never move work to the CPU because the CPU is faster. A GPU that is fast enough is what counts, and it saves CPU and battery.
   - The only exception is a GPU that gives wrong results, such as wrong frames after a seek. Name it in `GpuDecoder::why_not` and gotchas.md.
10. **The user's settings reach the encoder as they are.** No hidden rescaling, clamping or substitution of quality, speed, codec or any preference. Where an encoder can't take a value, ask before adding a conversion, and show it where the user sets it and in the export's report.
11. **The docs aren't a history. Git is.** The `.md` files say what is true now, as briefly as they can.
   - No progress notes, past attempts, "before, X did Y" or why a mistake happened. That goes in commit messages and PR descriptions.
   - A todo file says what is open, not what got done.
   - A PR that touches a doc also cuts the stale text in the parts it touches.
12. **GUI text is short.** A window's title says what it does, so no window opens with a paragraph about itself.
   - Labels and buttons are a few words, in Title Case. A tooltip only where the label isn't enough. A line under the controls only for what changes the result: a download, a warning, files to transcribe first.
   - No string in `crates/vidcut/src/gui/` has more than 12 words; `scripts/check.sh` enforces it. The details go in the docs.
   - Windows use `gui/dialog.rs`: labeled rows, two or three choices as a switch, rarely needed settings under Advanced, and Cancel with one main button at the bottom right.

## Quality gates

These are mandatory.

| When | Command | Runs |
|---|---|---|
| Before **every commit** (enforced by `.githooks/pre-commit`) | `make check` (`scripts/check.sh`); for `.md` files alone, `scripts/check-docs.sh` | no binary files in git, no comments in `crates/` (rule 2), GUI text of 12 words at most (rule 12), the docs' names and links and no conflict marks (`scripts/check-docs.sh`), `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, unit tests (`--lib --bins`) |
| After a **bigger change** | `make check-full` (`scripts/check.sh full`) | the above plus the examples, benches and doc tests |

**Enable the hook once per clone:** `make hooks` (`git config core.hooksPath .githooks`).

**On Windows**, which has no make, the scripts run from Git Bash, with `FFMPEG_DIR` set (README.md, Requirements).

**CI** (`.github/workflows/check.yml`) runs only the fast gate, on Linux, Windows and macOS, on pull requests and pushes to master. `.github/workflows/ffmpeg.yml` builds against the newest FFmpeg release and master weekly and on demand.

## Docs

Each fact lives in one doc; the others link to it. The design docs name what a reader greps for (types, modules, request methods, thread names), not tunable numbers, and `scripts/check-docs.sh` checks that every name in backticks in them exists in `crates/`.

- [README.md](README.md): what vidcut does, requirements, build.
- [docs/cli.md](docs/cli.md): the CLI guide (times, JSON output, exit codes, scripting, running GUIs); every command and flag is in `vidcut --help`.
- [docs/cut-page-spec.md](docs/cut-page-spec.md): the behavioral spec; [docs/deviations.md](docs/deviations.md): where vidcut differs.
- [docs/design/](docs/design/README.md): how vidcut is built, with charts; start there.
- [docs/tools.md](docs/tools.md): debug and test tools; [docs/gotchas.md](docs/gotchas.md): FFmpeg, driver and library behavior vidcut works around.
- [docs/roadmap.md](docs/roadmap.md): what comes next; [docs/todo/](docs/todo/): open work, one file per item.

## Cloud agents

Ubuntu 26.04's FFmpeg 8.0 is the one `pkg-config` finds. FFmpeg 8.1.3 and the newest release are already built at `/opt/ffmpeg/8.1` and `/opt/ffmpeg/9.0.2`. [docs/tools.md](docs/tools.md) says how `scripts/with-ffmpeg.sh` selects one.
