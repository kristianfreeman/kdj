# The image build's containers (see build.sh): Debian arm64 of the Pi
# image's release, with what building the deck and working on a disk image
# need already installed. Docker keeps it, so builds don't install these
# every time (the scripts install anything missing, so they work without).
ARG CODENAME=trixie
FROM debian:${CODENAME}
RUN apt-get update -qq \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
        build-essential pkg-config git curl ca-certificates \
        libsdl2-dev libasound2-dev cmake clang libclang-dev \
        fdisk e2fsprogs zerofree xz-utils mount util-linux systemd-container \
    && rm -rf /var/lib/apt/lists/*
