# Frame interpolation

**When:** now, UHD first.

Retime Process makes the frames between source frames ([deviation X93](../deviations.md#x93-retime-process-has-ai-models-in-the-place-of-optical-flow), [gpu](../design/gpu.md#frame-interpolation)). These parts are still to do.

- **UHD:** EMA-VFI keeps 32-bit values in every buffer, and all its buffers stay at the same time. A 1080p frame needs 12 GB of GPU memory, and a UHD frame does not fit in 16 GB.
- **Names:** RIFE 4.25 and EMA-VFI then get names such as AI Fast and AI Best, as Super Scale has.
- **Speed:** the convolution kernel does not use the tensor cores.
- **The viewer:** it shows the frames that an export makes. With EMA-VFI, a frame can take seconds.
- **Backward clips:** a clip played backward shows the nearest frames.
- **HDR:** Retime Process refuses HDR pictures. The frames go through 8-bit RGB.
- **Super Scale:** with an AI upscaler, a made frame is enlarged as with Classic.
- **OpenTimelineIO:** an OTIO file does not keep the Retime Process of a clip yet. How Resolve writes it in its files is not measured.
