# FFmpeg master: ffmpeg-sys-next can't bind hwcontext_cuda.h

**When:** before the next FFmpeg release that has it (after 9.0). Until then the `master` target of `scripts/ffmpeg-check.sh` and the master job of `.github/workflows/ffmpeg.yml` fail.

FFmpeg's master added CUarray surface pools to `libavutil/hwcontext_cuda.h` (commit ef3dcf4ea6): fields of the CUDA types `CUarray` and `CUDA_ARRAY3D_DESCRIPTOR`. ffmpeg-sys-next (9.0.1) binds that header without the CUDA SDK, declaring only `CUcontext` and `CUstream` itself (`hwcontext_wrapper.h`), so bindgen stops with "unknown type name 'CUarray'".

The fix belongs in ffmpeg-sys-next: stand-ins for the two types next to the others. Then update the crate here and check `scripts/ffmpeg-check.sh master`.
