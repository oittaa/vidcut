# GPU frames

- Frames in GPU memory to the VA-API, AMF and QSV encoders.
- Frames in GPU memory to the Vulkan ProRes encoder.
- One frame backend for each place: memory, CUDA, VA-API, Direct3D 11, VideoToolbox and Vulkan.
- A test, for each GPU and FFmpeg, of which encoders take the frames of which places.
- The light meter on the GPU for SDR pictures in an HDR export.
- The light meter on Direct3D 11, VA-API and VideoToolbox pictures: not checked against the CPU.
