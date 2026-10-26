# Frame interpolation

**When:** after the owner compares the three models.

Retime Process makes the frames between source frames ([deviation X93](../deviations.md#x93-retime-process-has-ai-models-in-the-place-of-optical-flow), [gpu](../design/gpu.md#frame-interpolation)). These parts are still to do.

- **Fewer models:** the owner compares RIFE 4.25, EMA-VFI and VFIMamba on test clips. vidcut then keeps fewer of them.
  - The models that stay then get names such as AI Fast and AI Best, as Super Scale has.
- **UHD:** EMA-VFI and VFIMamba keep 32-bit values in every buffer. A UHD frame then needs about four times the memory of a 1080p frame.
  - Their buffers at UHD are not measured yet. 16-bit values or tiles of the full-size stages can make them smaller.
- **Speed:**
  - VFIMamba's scan runs one thread for each channel of each direction over the whole sequence. A scan in chunks can use more threads.
  - The convolution kernel does not use the tensor cores.
- **The viewer:** it shows the frames that an export makes. With EMA-VFI and VFIMamba, a frame can take seconds.
- **Backward clips:** a clip played backward shows the nearest frames.
- **HDR:** Retime Process refuses HDR pictures. The frames go through 8-bit RGB.
- **Super Scale:** with an AI upscaler, a made frame is enlarged as with Classic.
- **OpenTimelineIO:** an OTIO file does not keep the Retime Process of a clip yet. How Resolve writes it in its files is not measured.
