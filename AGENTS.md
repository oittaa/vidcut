# AGENTS.md

Guide for AI agents (and humans) who work on this repository.

vidcut is a video cutting tool in Rust. Its model is the Cut page of DaVinci Resolve. [README.md](README.md) tells what vidcut does and how to build it. [docs/design/](docs/design/README.md) tells how it is built.

## Ground rules

1. **The Cut page is the spec.** The default behavior is the behavior of Resolve's Cut page. See [docs/cut-page-spec.md](docs/cut-page-spec.md), with the manual's page numbers.
   - Make a different behavior only when it is clearly better.
   - Record each difference in [docs/deviations.md](docs/deviations.md): Resolve's behavior and page, our behavior, and the reason.
2. **The code does not name DaVinci Resolve, and it has no comments.**
   - Nothing in `crates/` names Resolve or the pages of its manual.
   - Nothing that vidcut shows or prints names Resolve: GUI text, `--help`, the schema, MCP text, errors.
   - The only exception is file-format data that other editors need, for example the OTIO metadata keys that they read.
   - `crates/` has no comments, with three exceptions:
     - The schema's doc comments. They are shipped text: `vidcut schema`, `--help` and MCP show them.
     - One-line `// SAFETY:` comments.
     - The `# Safety` section that clippy requires on a public unsafe fn, one line under its heading.
   - `scripts/check.sh` enforces these rules.
   - The reasons for how the code is built go in the design docs in `docs/design/`.
3. **Everything is scriptable.** The CLI, IPC and MCP can do every GUI action.
4. **No media in git.**
   - Test media, generated fixtures and the manual are in `$VIDCUT_TEST_DATA` (default `~/Downloads/vidcut-tmp`). This is the one place for test data.
   - Test outputs go to temp dirs.
   - `scripts/check.sh` (the pre-commit hook and CI) refuses any binary file, whatever its name. Thus `.gitignore` lists no file types.
   - `tests/media-manifest.toml` lists the real-world clips (URL, license, sha256). `scripts/fetch-test-media.sh` gets them.
5. **No ffmpeg or ffprobe subprocesses.** Use `vidcut-av`, which uses the FFmpeg libraries, for exports and for the preview.
6. **License:** MIT.
7. **FFmpeg:** from 8.0 (the latest Ubuntu LTS) to the newest release. Do not write code for FFmpeg before 8.0.
8. **A unit test is a unit test.** It has one behavior, a fixed input, and the output it must give, written out. Example: `assert_eq!(sha("test text"), "abcd")`.
   - It does not implement the code a second time to compute the expected output.
   - It does not test many things at once, and it is not 100 lines or more.
   - Check by hand what needs more than that. No tests that open a window, play real media or depend on timing.
9. **The GPU first, where the user lets it work.** With `gpu.acceleration` and `gpu.decoding` on, decoding and encoding run on the GPU in every path where the GPU gives correct results, H.264 too.
   - Do not move work to the CPU because the CPU is faster. A GPU that is fast enough is sufficient, and it saves CPU time and battery.
   - The only exception is a GPU that gives wrong results, for example wrong frames after a seek. Name it in `GpuDecoder::why_not` and in gotchas.md.
10. **The user's settings go to the encoder as they are.**
    - Do not rescale, clamp or replace quality, speed, codec or any other preference without telling the user.
    - If an encoder cannot take a value, ask before you add a conversion.
    - Show the conversion where the user sets the value and in the export's report.
11. **The docs state facts about the current code. They are not a history: git is the history.**
    - Write the `.md` files in ASD-STE100 Simplified Technical English (STE). Use short sentences, one fact in each sentence, active voice and the present tense. Use lists for complex text, and use the same word for the same thing.
    - A doc tells what the code does now and why. A todo file tells what is still to do and why.
    - Do not write progress notes, earlier attempts, "before, X did Y" or why a mistake occurred. Put them in commit messages and PR descriptions.
    - A PR that changes a doc also removes the stale text in the parts that it changes.
    - `scripts/check-docs.sh` refuses a sentence of more than 25 words and a line of more than 300 characters.
12. **GUI text is short.** The title of a window tells what the window does. Thus no window opens with a paragraph about itself.
    - Labels and buttons have a few words, in Title Case.
    - Use a tooltip only where the label is not sufficient, for example the full text of a label that is cut off. A tooltip never repeats the label.
    - Put a line under the controls only for things that change the result: a download, a warning, files to transcribe first.
    - No string in `crates/vidcut/src/gui/` has more than 12 words. `scripts/check.sh` enforces this. The details go in the docs.
    - Windows use `gui/dialog.rs`: labeled rows, a switch for two or three choices, and rarely used settings under Advanced. Cancel and one main button are at the bottom right.

## Quality gates

These gates are mandatory.

| When | Command |
|---|---|
| Before **each commit** (`.githooks/pre-commit` enforces it) | `make check` (`scripts/check.sh`). For `.md` files only, `scripts/check-docs.sh`. |
| After a **bigger change** | `make check-full` (`scripts/check.sh full`) |

`make check` runs these checks:

- No binary files in git.
- No comments in `crates/` (rule 2).
- GUI text of 12 words at most (rule 12).
- The docs: names, links, style and no conflict marks (`scripts/check-docs.sh`).
- `cargo fmt --check`.
- `cargo clippy --workspace --all-targets -D warnings`.
- The unit tests (`--lib --bins`).

`make check-full` also runs the examples, the benches and the doc tests.

**Turn on the hook once in each clone:** `make hooks` (`git config core.hooksPath .githooks`).

**On Windows**, which has no make, run the scripts from Git Bash, with `FFMPEG_DIR` set (README.md, Requirements).

**CI** (`.github/workflows/check.yml`) runs only the fast gate, on Linux, Windows and macOS. It runs on pull requests and on pushes to master. `.github/workflows/ffmpeg.yml` builds against the newest FFmpeg release and FFmpeg master, each week and on demand.

## Docs

Each fact is in one doc. The other docs link to it.

The design docs name the things that a reader searches for: types, modules, request methods and thread names. They do not give tunable numbers. `scripts/check-docs.sh` checks that each name in backticks in them is in `crates/`.

- [README.md](README.md): what vidcut does, the requirements, the build.
- [docs/cli.md](docs/cli.md): the CLI guide (times, JSON output, exit codes, scripts, running GUIs). `vidcut --help` gives each command and flag.
- [docs/cut-page-spec.md](docs/cut-page-spec.md): the behavior spec. [docs/deviations.md](docs/deviations.md): where vidcut is different.
- [docs/design/](docs/design/README.md): how vidcut is built, with charts. Start there.
- [docs/tools.md](docs/tools.md): debug and test tools. [docs/gotchas.md](docs/gotchas.md): FFmpeg, driver and library behavior that vidcut works around.
- [docs/roadmap.md](docs/roadmap.md): what comes next. [docs/todo/](docs/todo/): open work, one file for each item.

## Cloud agents

`pkg-config` finds FFmpeg 8.0 from Ubuntu 26.04. FFmpeg 8.1.3 and the newest release are already built at `/opt/ffmpeg/8.1` and `/opt/ffmpeg/9.0.2`. [docs/tools.md](docs/tools.md) tells how `scripts/with-ffmpeg.sh` selects one.
