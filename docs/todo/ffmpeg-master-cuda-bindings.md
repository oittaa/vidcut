# FFmpeg master: ffmpeg-sys-next can't bind hwcontext_cuda.h

**When:** before the next FFmpeg release that has it (after 9.0). Until then the `master` target of `scripts/ffmpeg-check.sh` and the master job of `.github/workflows/ffmpeg.yml` fail.

**Why:**

- On FFmpeg master, `libavutil/hwcontext_cuda.h` has CUarray surface pools (commit ef3dcf4ea6).
- The pools add fields of the CUDA types `CUarray` and `CUDA_ARRAY3D_DESCRIPTOR`.
- ffmpeg-sys-next (9.0.1) binds that header without the CUDA SDK. It declares only `CUcontext` and `CUstream` itself (`hwcontext_wrapper.h`).
- Thus bindgen stops with "unknown type name 'CUarray'".

**To do:**

1. Make the fix in ffmpeg-sys-next: stand-ins for the two types, next to the others.
2. Update the crate in vidcut.
3. Run `scripts/ffmpeg-check.sh master`.
