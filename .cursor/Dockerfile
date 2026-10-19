FROM ubuntu:26.04

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

# n8.1.3. 9.0.2 comes from scripts/ffmpeg/release.env via build-ffmpeg.sh.
ARG FFMPEG_8_1=1041abdc962f4cc4f394aa8de9dc5236c0c3b9e7

COPY scripts/ffmpeg/packages.txt scripts/ffmpeg/build-ffmpeg.sh scripts/ffmpeg/release.env /usr/local/share/vidcut/
COPY scripts/with-ffmpeg.sh /usr/local/bin/with-ffmpeg

RUN apt-get update \
    && grep -v '^\s*\(#\|$\)' /usr/local/share/vidcut/packages.txt \
        | xargs apt-get install -y --no-install-recommends \
    && apt-get install -y --no-install-recommends \
        sudo \
        rustup \
        libavcodec-dev \
        libavformat-dev \
        libavfilter-dev \
        libavutil-dev \
        libswscale-dev \
        libswresample-dev \
        libavdevice-dev \
        libxkbcommon-dev \
        libxkbcommon-x11-0 \
        libwayland-dev \
        libx11-dev \
        libxcb1-dev \
        libxcb-render0-dev \
        libxcb-shape0-dev \
        libxcb-xfixes0-dev \
        libvulkan1 \
        mesa-vulkan-drivers \
    && rm -rf /var/lib/apt/lists/* \
    && chmod 755 /usr/local/bin/with-ffmpeg \
    && . /usr/local/share/vidcut/release.env \
    && /usr/local/share/vidcut/build-ffmpeg.sh release "/opt/ffmpeg/${FFMPEG_RELEASE}" \
    && VIDCUT_FFMPEG_VERSION=8.1.3 /usr/local/share/vidcut/build-ffmpeg.sh "$FFMPEG_8_1" /opt/ffmpeg/8.1 \
    && useradd --create-home --shell /bin/bash --groups sudo ubuntu \
    && printf 'ubuntu ALL=(ALL) NOPASSWD:ALL\n' > /etc/sudoers.d/ubuntu \
    && chmod 440 /etc/sudoers.d/ubuntu \
    && mkdir -p /usr/local/rustup /usr/local/cargo \
    && chown -R ubuntu:ubuntu /usr/local/rustup /usr/local/cargo \
    && printf 'export RUSTUP_HOME=/usr/local/rustup\nexport CARGO_HOME=/usr/local/cargo\nexport PATH="/usr/local/cargo/bin:$PATH"\n' > /etc/profile.d/rust.sh

USER ubuntu
WORKDIR /workspace
