# vidcut

vidcut is a video editor for Linux, Windows and macOS. Each action in its window is also a CLI command, a JSON-RPC call and an MCP tool.

**Status:** early development.

## Features

- Timeline editing: insert, append, overwrite, trim and ripple delete, on several tracks.
- Dissolves, wipes and cross fades.
- Clip speed, freeze frames and AI frames between the source frames (Retime Process).
- AI upscaling (Super Scale).
- Transform, crop, Dynamic Zoom, opacity and composite modes, with keyframes.
- Titles.
- The Mixer, volume keyframes and Voice Isolation.
- Subtitles from speech. SRT, WebVTT and TTML in and out.
- Ripple Delete Silence and Detect Scene Cuts.
- Export with GPU encoding and HDR.
- OpenTimelineIO in and out.

The AI runs on this computer. vidcut downloads each model the first time that it needs it, after it asks you. `vidcut models list` lists the models and their sizes.

## Requirements

**Linux** (Ubuntu 26.04 or similar):

```bash
sudo apt install libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libswscale-dev libswresample-dev libavdevice-dev libasound2-dev libclang-dev rustup
```

**macOS:** put `$(brew --prefix rustup)/bin` on `PATH`, and `$(brew --prefix ffmpeg-full)/lib/pkgconfig` on `PKG_CONFIG_PATH`.

```bash
brew install ffmpeg-full rustup
```

**Windows 10 or 11:** install Visual Studio 2022 with the C++ workload, then:

```bash
winget install BtbN.FFmpeg.GPL.Shared.9.0
```

```bash
winget install LLVM.LLVM
```

- Set `FFMPEG_DIR` to the FFmpeg folder, and put its `bin` on `PATH`.
- If the build does not find libclang, set `LIBCLANG_PATH` to `C:\Program Files\LLVM\bin`.

Then:

```bash
make doctor     # what the build needs, the GPUs and the encoders
make run        # build and start the GUI
make install    # copy vidcut into ~/.local/bin
```

- vidcut works with FFmpeg 8.0 to 9.0.
- On Windows, `cargo build --release` builds `target\release\vidcut.exe`.

## CLI

```bash
vidcut project create talk.vidcut
vidcut -p talk.vidcut media add cam.mp4
vidcut -p talk.vidcut edit append m1 --range 10s..2m
vidcut -p talk.vidcut export start talk.mp4 --preset youtube-1080p
```

- `vidcut --help` lists the commands. [docs/cli.md](docs/cli.md) tells more.
- With Claude Code: `claude mcp add vidcut -- vidcut mcp -p /path/to/talk.vidcut`.

## Files

| | Linux | Windows | macOS |
|---|---|---|---|
| Preferences, presets, themes | `~/.config/vidcut` | `%APPDATA%\vidcut` | `~/Library/Application Support/vidcut` |
| Cache and models | `~/.cache/vidcut` | `%LOCALAPPDATA%\vidcut\cache` | `~/Library/Caches/vidcut` |
| Undo history, recent projects | `~/.local/state/vidcut` | `%LOCALAPPDATA%\vidcut\state` | `~/Library/Application Support/vidcut/state` |

`vidcut reset` sets these folders back to a fresh install.

## Contributing

Read [AGENTS.md](AGENTS.md) first. [docs/design/](docs/design/README.md) tells how vidcut is built.

## License

- vidcut is MIT ([LICENSE](LICENSE)).
- The fonts Geist, Geist Mono and Space Grotesk are under the SIL Open Font License 1.1.
- The FFmpeg libraries are LGPL or GPL, by how they were built.
- transcribe.cpp, ggml and Silero VAD are MIT ([snakers4/silero-vad](https://github.com/snakers4/silero-vad/blob/5cd7945/LICENSE)).
- The speech models are GGUF files that handy-computer converted:
  - Qwen3-ASR 1.7B, Whisper large-v3 and MOSS-Transcribe-Diarize: Apache-2.0.
  - Parakeet Ultra ([moondream/parakeet-ultra](https://huggingface.co/moondream/parakeet-ultra)): CC BY 4.0.
  - Sortformer ([nvidia/diar_streaming_sortformer_4spk-v2.1](https://huggingface.co/nvidia/diar_streaming_sortformer_4spk-v2.1)): NVIDIA Open Model License.
- ureq, rustls and keyring are MIT or Apache-2.0.
