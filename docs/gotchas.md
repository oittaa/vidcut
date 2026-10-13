# Gotchas

Pitfalls not covered by the decision records ([decisions/](decisions/)).

## wgpu and egui

- **wgpu's default error handler panics.** A texture larger than the device allows, or a binding of the wrong format, takes the GUI down. Check sizes against `device.limits()` (`Screen::draw` draws black for a picture over the limit), and keep the shader's bindings and `Gpu::new`'s layout together.
- **egui-wgpu shows native textures as they are** (`Rgba8Unorm`, no sRGB conversion): the viewer's target holds gamma-encoded BT.709 RGB.

## FFmpeg

- **`VideoReader::seek` drops every frame before its target** (`min_pts`), including the keyframe it lands on. A reader that only wanted the keyframe (`VideoFrames::keyframe_time`) calls `from_keyframe` to decode on from it, or it seeks again. A seek decodes on to its target however far into the GOP it is: no keyframe lies between, so the usual one-second rule (seek again when far behind) doesn't apply.
- **Every seek of H.264 on NVDEC recreates the decoder.** FFmpeg's H.264 decoder forgets its context on a flush (`h264_decode_flush`), so the next keyframe goes through `get_format` again, which tears down the NVDEC decoder and makes another: a few hundred milliseconds. HEVC and AV1 keep theirs. A seek that flushes twice pays twice.
- **MP4 seeks by decode time.** With B-frames a keyframe decodes before its own presentation time, so a target just before a keyframe lands on that keyframe, after the target. `VideoReader::seek` sees that in the first packet and seeks again before decoding.
- **Test cuts against fixtures with B-frames and against real media**, not only small fixtures: open GOPs and x264's B-frames (a keyframe decoded before the frames shown just ahead of it) are where readers lose frames at cuts.
- **Set packet durations explicitly when muxing** from delayed encoders (x264 with B-frames). Otherwise MP4 edit lists drop the last frame.
- **Clear a decoded frame's picture type before encoding it** (`set_kind(picture::Type::None)`). Encoders take it as an order: the output copies the source's keyframes, and x264 warns about every B-frame it can't make.
- **FFmpeg silently drops an option an encoder doesn't have:** `avcodec_open2` leaves it in the dictionary and opens anyway (`av1_vaapi` has no `qp`; its quantizer is `global_quality`). `options_are_the_encoders_own` (`encode.rs`, `proxy.rs`) checks every option vidcut sets.
- **`AVHWDeviceContext.user_opaque` belongs to FFmpeg** for devices `av_hwdevice_ctx_create` makes: VA-API keeps its DRM file there and frees it through that pointer.
- **Take a frame's planes once, not per sample, and don't call `f64::round` in per-pixel loops.** ffmpeg-next's `frame::Video::data()` and `stride()` look up the pixel format on every call, and `f64::round` is a library call on baseline x86-64 (`light::Samples` rounds a non-negative value by adding a half and truncating).

## HDR and color

- **Matroska's demuxer exports content light levels only when MaxCLL and MaxFALL are both non-zero**, and FFmpeg's decoders put a container's HDR values on every frame unless the bitstream has its own. Read a file's values from its first decoded frame (`hdr::of_frame`).
- **FFmpeg's HEVC decoder puts the last HDR10+ it read on every frame after it** (8.0 and 9.0), in decode order, so a frame without its own can't be told from one with it. libdav1d's AV1 frames carry only their own. A test reading HDR10+ back from HEVC output sees a missing one as the one before it.
- **An untagged picture's matrix follows from its size** (`color::untagged_matrix`: BT.709 from 1280 wide or over 576 lines). A copy scaled on the GPU must carry the matrix of its source (`color::keep_matrix`), or a shrunk 720p picture reads as BT.601.

## GPU decoders and drivers

- **Never give libplacebo the same Vulkan frame twice at once.** It takes a frame at the semaphore value FFmpeg's frame says and gives it back at the next one, so a frame held twice ends a step ahead of anything the GPU signals: FFmpeg, freeing it, waits for that value forever, and a transfer from CUDA can read its picture before it is there (`cuda::Blacks`, export.md §5).
- **Intel's QSV runtime opens VA-API displays of its own** to list what each GPU can do, and libva prints four info lines on stderr for each. `main` sets `LIBVA_MESSAGING_LEVEL=1` (errors only) on Linux unless it is set.
- **FFmpeg's copy-down of Intel's VA-API frames is slow:** it reads a driver-made copy of the surface (`vaGetImage`); mapping the surface itself (`vaDeriveImage`) can copy twice as fast. But on Gen7–Gen9 the mapped memory is uncached, and on a discrete card (Arc) it is across the bus. `hw::download` times both ways on the first frames of each display and keeps the faster (`hw::Ways`); it recognizes Intel's driver by libva's vendor string.
- **AMD's video processor clips levels outside video range when told studio range** (below black and above white come out at 16 and 235, chroma 16 and 240). Told full range in and out, it keeps every level, so `hw::d3d11::Scaler` says full range where DXGI has such a color space. PQ has only studio range, so PQ is clipped there.
- **AMD's Direct3D 11 decoder loses its way in H.264 sought into an open GOP:** started at a keyframe that isn't IDR, it decodes a few frames right, then repeats one picture. From the start of the file it decodes right. The CPU decodes H.264 there (`GpuDecoder::decodes`); HEVC, AV1 and VP9 stay on the GPU. `seeks_into_long_open_gops_show_their_frames` (`player_gpu.rs`) checks every GPU decoder.
- **FFmpeg's `scale_d3d11` fails on AMD's driver:** its output textures are also bound for the video encoder (`D3D11_BIND_VIDEO_ENCODER`), which the driver refuses for NV12 and P010 (`E_INVALIDARG`). It also has no layout unless given `format=`, and scales the decoder's padding into the picture (1080p decodes into 1088 lines). `hw::d3d11::Scaler` drives the video processor itself.

## Audio

- **cpal's lookup of a device by its id misses ALSA's** (0.18): `device_by_id` adds a device number to the id (`sysdefault:CARD=PCH` becomes `sysdefault:CARD=PCH,DEV=0`), while listed devices keep their ids without it, so a chosen device would silently play on the default. `CpalOutput` finds the device among those listed, by its id as listed (`output_by_id`).

## Paths and IPC

- **`canonicalize` on Windows gives verbatim paths** (`\\?\C:\…`), which don't compare equal to the same path as people write it and leak into project files and messages. Use `vidcut_core::dirs::canonical`, which keeps the usual form where it names the same file.
- **Project files keep relative paths with `/`** on every system (`store::portable`) and read them back with the system's separator, so a project folder moves between Linux and Windows. Compare paths as `Path`s, not strings: on Windows `/` and `\` both separate.
- **A Unix socket on Windows can't be woken by a shutdown:** a read blocked in one thread doesn't end when another thread shuts the socket down. Live control uses named pipes there, read with overlapped calls (`ipc::transport`).

## macOS

- **`poll` reports a hang-up only for the events asked for:** with `events` 0 it reports nothing, where Linux always reports `POLLHUP` and `POLLERR`. Asked for `POLLIN`, a Unix socket reports a hang-up once the peer stops writing; asked for `POLLOUT`, once the peer has closed (`ipc::transport::Stream::peer_gone`).
- **Threads don't inherit a QoS class:** a thread started from a utility-class thread runs in the default class, like Windows's priorities and unlike Linux's idle class (`threads.rs`).
