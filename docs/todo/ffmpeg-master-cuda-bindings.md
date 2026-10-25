# FFmpeg master: ffmpeg-sys-next can't bind hwcontext_cuda.h

**When:** before the next FFmpeg release that has it (after 9.0). Until then the `master` target of `scripts/ffmpeg-check.sh` and the master job of `.github/workflows/ffmpeg.yml` fail.

**Why:**

- On FFmpeg master, `libavutil/hwcontext_cuda.h` has CUarray surface pools (commit ef3dcf4ea6).
- The pools add fields of the CUDA types `CUarray` and `CUDA_ARRAY3D_DESCRIPTOR`.
- ffmpeg-sys-next (9.0.1) binds that header without the CUDA SDK. It declares only `CUcontext` and `CUstream` itself (`hwcontext_wrapper.h`).
- Thus bindgen stops with "unknown type name 'CUarray'".
- FFmpeg 9.0.2, the newest release, does not have the pools. Thus the release job passes.
- vidcut does not pin an old version of the crate. A new release of ffmpeg-sys-next must have the fix.

**To do:**

1. Get the fix into ffmpeg-sys-next.
   - Pull request [zmwangx/rust-ffmpeg-sys#139](https://github.com/zmwangx/rust-ffmpeg-sys/pull/139) adds stand-ins for the two types, next to the others.
   - It waits for a review, a merge and a release.
2. Update the crate in vidcut.
3. Run `scripts/ffmpeg-check.sh master`.
