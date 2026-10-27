# Frame interpolation

**When:** after zero copy on macOS.

Retime Process makes the frames between source frames ([deviation X93](../deviations.md#x93-retime-process-has-ai-models-in-the-place-of-optical-flow), [gpu](../design/gpu.md#frame-interpolation)). These parts are still to do.

- **Memory:** AI Best keeps 32-bit values. An export of a UHD clip needs 9.5 GB of GPU memory, and a 1080p clip 4.2 GB.
  - 16-bit values for the shallow features and the attention buffers can make this smaller.
- **Zero copy on macOS:** [macos.md](macos.md#pictures-on-the-gpu).
- **The first frame on macOS:** the first frame of each size takes some seconds while macOS starts its session. The viewer waits that long.
- **Speed:** the convolution kernel does not use the tensor cores.
- **The viewer:** it shows the frames that an export makes. With AI Best, a frame can take seconds.
- **Backward clips:** a clip played backward shows the nearest frames.
- **HDR:** Retime Process refuses HDR pictures. The frames go through 8-bit RGB.
- **Super Scale:** with an AI upscaler, a made frame is enlarged as with Classic.
- **OpenTimelineIO:** an OTIO file does not keep the Retime Process of a clip yet. How Resolve writes it in its files is not measured.
