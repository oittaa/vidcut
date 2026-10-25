# vidcut

vidcut is a simple video cutting tool for Linux, Windows and macOS. It is fully scriptable.

It behaves like a Cut page ([spec](docs/cut-page-spec.md)). A script or an AI agent can do everything that you can do in its window.

**Status:** early development. It works, but expect rough edges.

## What it does

- **Editing:**
  - A media pool with bins, a source and timeline viewer, and the dual timeline.
  - Smart Insert, Append, Ripple Overwrite, Overwrite and Place On Top.
  - Split, ripple delete, trimming, the Trim Editor, Dynamic Trim and clip speed.
  - Dissolves and fades, several video and audio tracks, and a mixer.
  - Snapping, markers and undo.
- **Export:** Quick Export and an Export dialog, GPU encoding (NVIDIA, AMD, Intel, Apple), background jobs, HDR that follows the footage, and loudness normalization.
- **Automatic tools:** Ripple Delete Silence, Detect Scene Cuts and the Boring Detector.
- **Sound:**
  - Voice Isolation on a clip or a track. It runs on this computer and is free.
  - The Mixer, with volume, mute and solo.
  - A volume line on each clip, with keyframes and eases. Alt+click adds a keyframe.
- **Titles:** text titles and lower thirds on the video tracks. You edit them in the Inspector. The viewer and the export draw them the same.
- **Transform and Crop:** move, zoom, turn, flip and crop a clip's picture over the tracks below, with the viewer's handles (T, Shift+T). Use it for a picture in a picture, or for a punch-in that keeps the source's detail.
- **Opacity and soft edges:** fade a clip over the tracks below, and soften the edges of its crop. Both take keyframes.
- **Subtitles:**
  - Subtitle tracks.
  - Create Subtitles from Audio, which also tells who speaks. It runs on this computer's GPU, or with xAI or Google after vidcut asks you.
  - A track re-timed to the speech, in any language.
  - SRT, WebVTT and TTML in and out.
  - In an export: files beside it, streams in it, or burned into the picture.
- **Interchange:** OpenTimelineIO (`.otio`, `.otioz`, `.otiod`). These files also open as projects. Their subtitles go beside them as SRT.
- **Scriptable:** each action is a CLI command, a JSON-RPC call on the running app and an MCP tool.
- **Not included:** other effects, other titles, color grading, and transitions other than dissolves. When another editor's OTIO file has them, vidcut keeps them and writes them back.

Where vidcut differs from the Cut page: [docs/deviations.md](docs/deviations.md).

## Requirements

**Linux:** Ubuntu 26.04 (or similar) with FFmpeg:

```bash
sudo apt install libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libswscale-dev libswresample-dev libavdevice-dev libasound2-dev libclang-dev rustup
```

**Windows 10 or 11:** Visual Studio 2022 or its Build Tools with the C++ workload, LLVM, and an FFmpeg build with headers and import libraries. An example is BtbN's shared GPL build:

```bash
winget install BtbN.FFmpeg.GPL.Shared.9.0
```
```bash
winget install LLVM.LLVM
```

- Set `FFMPEG_DIR` to the FFmpeg folder (with `bin`, `include` and `lib`).
- Put its `bin` on `PATH`.
- If the build does not find libclang, set `LIBCLANG_PATH` to `C:\Program Files\LLVM\bin`.

**macOS:** with `$(brew --prefix rustup)/bin` on `PATH` and `$(brew --prefix ffmpeg-full)/lib/pkgconfig` on `PKG_CONFIG_PATH`:

```bash
brew install ffmpeg-full rustup
```

On a Mac, the full test gate and an app bundle are still open ([todo](docs/todo/macos.md)).

**All systems:**

- vidcut works with FFmpeg 8.0 to 9.0.
- It decodes on the GPU where it can (NVDEC, VA-API, Direct3D 11, VideoToolbox).
- At run time, it needs only the Vulkan loader on Linux (libvulkan1) and the Visual C++ runtime on Windows. Visual Studio installs the Visual C++ runtime.
- The GUI draws with Vulkan, Direct3D 12 or Metal.
- The GUI plays sound through ALSA (served by PipeWire or PulseAudio), WASAPI or Core Audio.
- Speech recognition uses the prebuilt library of [transcribe.cpp](https://github.com/handy-computer/transcribe.cpp), on Vulkan or Metal, and Silero VAD. The vidcut build does not build them.
  - vidcut downloads them into its cache folder the first time that it uses speech, after it asks you. In scripts, use `download`.
  - The library is 18 MB on Windows, 23 MB on Linux and 2.5 MB on a Mac with Apple silicon. Silero VAD is 1.2 MB.
  - vidcut checks each download against its SHA-256.
- Voice Isolation runs DPDFNet on the CPU. vidcut downloads DPDFNet (10 MB) in the same way, the first time that you turn Voice Isolation on.
- `rust-toolchain.toml` pins the Rust version. rustup gets it on the first build.

## Build and run

```bash
make doctor                 # what a build still needs, then the GPUs and encoders
make                        # build
make run                    # the GUI
make run P=talk.vidcut      # the GUI with a project (created if missing)
make install                # vidcut into ~/.local/bin
```

`make help` lists the other targets. On Windows, `cargo build --release` builds `target\release\vidcut.exe`.

## Try the CLI

```bash
vidcut project create talk.vidcut
vidcut -p talk.vidcut media add cam.mp4
vidcut -p talk.vidcut edit append m1 --range 10s..2m
vidcut -p talk.vidcut export start talk.mp4 --preset youtube-1080p
```

- `vidcut --help` lists each command. `vidcut <command> --help` lists the flags of a command.
- While a GUI has the project open, these commands run inside the GUI.
- [docs/cli.md](docs/cli.md) tells about times, JSON output, exit codes and scripts.
- With Claude Code: `claude mcp add vidcut -- vidcut mcp -p /path/to/talk.vidcut`.

## Files

| Linux | Windows | macOS | Holds |
|---|---|---|---|
| `~/.config/vidcut` | `%APPDATA%\vidcut` | `~/Library/Application Support/vidcut` | preferences, presets, themes |
| `~/.cache/vidcut` | `%LOCALAPPDATA%\vidcut\cache` | `~/Library/Caches/vidcut` | thumbnails, waveforms, measurements, encoder detection, shaders, speech models, paid speech replies |
| `~/.local/state/vidcut` | `%LOCALAPPDATA%\vidcut\state` | `~/Library/Application Support/vidcut/state` | undo history, recent projects |
| `$XDG_RUNTIME_DIR/vidcut` | `%TEMP%\vidcut` | `$TMPDIR/vidcut-UID` | running GUIs |

- `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME` and `XDG_RUNTIME_DIR` move these folders, on each system.
- `vidcut reset` (or File > Preferences, Reset vidcut…) sets the first three folders back to the state of a fresh install.
  - It works only while no other vidcut GUI runs.
  - Your presets, themes and looks stay, unless you add `--saved`. A proxy folder that you set also stays.
  - `--dry-run` lists what it would remove.
- Set the preferences in File > Preferences or with `vidcut preferences`.
- The keyboard shortcuts are in Help (F1).
- Themes are TOML files. [starship.toml](crates/vidcut/themes/starship.toml) tells about each key.
- The `omarchy` theme takes its colors from the current Omarchy theme, and follows a change of it.
  - Without that desktop, it is Tokyo Night.
  - Without that desktop and while the computer is in light mode, it is Catppuccin Latte.

## Contributing

Read [AGENTS.md](AGENTS.md) first: the rules, the quality gates and the docs.

- [docs/design/](docs/design/README.md) tells how vidcut is built.
- [docs/roadmap.md](docs/roadmap.md) and [docs/todo/](docs/todo/) tell what comes next.

## License

- The source code of vidcut is MIT ([LICENSE](LICENSE)).
- vidcut carries the fonts Geist, Geist Mono and Space Grotesk. They are under the SIL Open Font License 1.1.
- vidcut links the FFmpeg libraries. They are LGPL or GPL, which depends on how they were built. Binaries distributed with them must obey their licenses.
- transcribe.cpp and ggml are MIT.
- The Silero VAD model is MIT, copyright (c) 2020-present Silero Team ([snakers4/silero-vad](https://github.com/snakers4/silero-vad/blob/5cd7945/LICENSE)). handy-computer converted it to GGUF.
- The speech models that vidcut downloads are GGUF files that handy-computer converted:
  - Qwen3-ASR 1.7B and Whisper large-v3, under Apache-2.0.
  - Parakeet Ultra by Moondream ([moondream/parakeet-ultra](https://huggingface.co/moondream/parakeet-ultra)), under CC BY 4.0.
  - NVIDIA's Sortformer speaker model ([nvidia/diar_streaming_sortformer_4spk-v2.1](https://huggingface.co/nvidia/diar_streaming_sortformer_4spk-v2.1)), under the NVIDIA Open Model License.
  - MOSS-Transcribe-Diarize ([OpenMOSS-Team/MOSS-Transcribe-Diarize](https://huggingface.co/OpenMOSS-Team/MOSS-Transcribe-Diarize)), under Apache-2.0.
- vidcut talks HTTPS to xAI and Google with ureq and rustls (MIT or Apache-2.0).
- vidcut reads keys from the system keychain with keyring (MIT or Apache-2.0).
