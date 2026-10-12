# syntax=docker/dockerfile:1
# Images for building and testing vidcut against one FFmpeg each; run them
# with scripts/ffmpeg-check.sh, which mounts the source and the caches.
#
#   lts    Ubuntu 26.04 with its own FFmpeg 8.0 and libmpv (the baseline)
#   built  Ubuntu 26.04 with FFmpeg built from source (build arg FFMPEG:
#          `release` for release.env, or a commit of FFmpeg's git)
#
# The build context is scripts/ffmpeg/; RUST is the channel in
# rust-toolchain.toml. FFmpeg builds in a stage of its own, so a new Rust
# toolchain doesn't build it again, nor a new FFmpeg fetch Rust.

ARG UBUNTU=ubuntu:26.04

FROM ${UBUNTU} AS os
ENV DEBIAN_FRONTEND=noninteractive
# The container runs as the caller's user, who has no home in the image.
# New volumes take the modes of these directories, so that user can write
# the caches.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential ca-certificates curl git pkg-config libclang-dev rustup \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -m 1777 /home/vidcut /cache /cache/cargo /cache/target /data
# No debug info, as in CI: target directories a third the size, faster
# links, and panics still name the file and line.
ENV HOME=/home/vidcut \
    RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/cache/cargo \
    CARGO_TARGET_DIR=/cache/target \
    CARGO_PROFILE_DEV_DEBUG=0 \
    VIDCUT_TEST_DATA=/data

# FFmpeg's build dependencies, which are also its run-time libraries.
FROM os AS ffmpeg-libs
COPY packages.txt /usr/local/share/vidcut/packages.txt
RUN apt-get update \
 && grep -v '^\s*\(#\|$\)' /usr/local/share/vidcut/packages.txt \
    | xargs apt-get install -y --no-install-recommends \
 && rm -rf /var/lib/apt/lists/*

FROM ffmpeg-libs AS ffmpeg
ARG FFMPEG=release
COPY build-ffmpeg.sh release.env /usr/local/share/vidcut/
RUN /usr/local/share/vidcut/build-ffmpeg.sh "$FFMPEG" /opt/ffmpeg

FROM ffmpeg-libs AS built
ARG RUST
RUN test -n "$RUST" \
 && CARGO_HOME=/tmp/cargo rustup toolchain install "$RUST" --profile minimal --component rustfmt,clippy \
 && chmod -R a+rX "$RUSTUP_HOME" && rm -rf /tmp/cargo
COPY --from=ffmpeg /opt/ffmpeg /opt/ffmpeg
# No libmpv: Ubuntu's links Ubuntu's FFmpeg 8.0, which would be a second
# FFmpeg in the process. The tests that need libmpv skip.
ENV PATH=/opt/ffmpeg/bin:$PATH \
    PKG_CONFIG_PATH=/opt/ffmpeg/lib/pkgconfig \
    LD_LIBRARY_PATH=/opt/ffmpeg/lib
COPY entrypoint.sh /usr/local/bin/vidcut-entrypoint
ENTRYPOINT ["vidcut-entrypoint"]

FROM os AS lts
# AGENTS.md "Build and run" (rustup and libclang are in `os`).
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev libswscale-dev \
      libswresample-dev libavdevice-dev libasound2-dev libmpv2 \
 && rm -rf /var/lib/apt/lists/*
ARG RUST
RUN test -n "$RUST" \
 && CARGO_HOME=/tmp/cargo rustup toolchain install "$RUST" --profile minimal --component rustfmt,clippy \
 && chmod -R a+rX "$RUSTUP_HOME" && rm -rf /tmp/cargo
COPY entrypoint.sh /usr/local/bin/vidcut-entrypoint
ENTRYPOINT ["vidcut-entrypoint"]
