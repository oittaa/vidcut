FROM ubuntu:26.04

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        git \
        make \
        pkg-config \
        sudo \
        xz-utils \
        libclang-dev \
        rustup \
        libavcodec-dev \
        libavformat-dev \
        libavfilter-dev \
        libavutil-dev \
        libswscale-dev \
        libswresample-dev \
        libavdevice-dev \
        libasound2-dev \
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
    && useradd --create-home --shell /bin/bash --groups sudo ubuntu \
    && printf 'ubuntu ALL=(ALL) NOPASSWD:ALL\n' > /etc/sudoers.d/ubuntu \
    && chmod 440 /etc/sudoers.d/ubuntu \
    && mkdir -p /usr/local/rustup /usr/local/cargo \
    && chown -R ubuntu:ubuntu /usr/local/rustup /usr/local/cargo \
    && printf 'export RUSTUP_HOME=/usr/local/rustup\nexport CARGO_HOME=/usr/local/cargo\nexport PATH="/usr/local/cargo/bin:$PATH"\n' > /etc/profile.d/rust.sh

USER ubuntu
WORKDIR /workspace
