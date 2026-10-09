#!/bin/bash
# Runs in a Debian arm64 container of the same release as the image (see
# build.sh): builds the deck release binary against that release's glibc
# and SDL2, with exactly setup.sh's build packages, and copies it to /out.
#
# Mounts: /src the repository (read-only), /cache a Docker volume (rustup,
# cargo registry, target dir), /out where the binary goes, /work a tmpfs.
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
export RUSTUP_HOME=/cache/rustup CARGO_HOME=/cache/cargo
# shellcheck source=/dev/null
codename=$(. /etc/os-release && echo "$VERSION_CODENAME")
export CARGO_TARGET_DIR=/cache/target-$codename

apt-get update -qq
apt-get install -y -qq --no-install-recommends \
    build-essential pkg-config git curl ca-certificates \
    libsdl2-dev libasound2-dev cmake clang libclang-dev >/dev/null
if [ ! -x "$CARGO_HOME/bin/cargo" ]; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
        sh -s -- -y --no-modify-path --profile minimal --default-toolchain none >/dev/null
fi
export PATH=$CARGO_HOME/bin:$PATH

# Copy the sources (not target/) so the build never writes to the Mac's tree;
# tar keeps modification times, so rebuilds stay incremental.
tar -C /src --exclude=./target --exclude=./.git -cf - . | tar -C /work -xf -
cd /work
rustc --version   # installs the toolchain pinned in rust-toolchain.toml
cargo build --release --locked -p kdj
install -m 755 "$CARGO_TARGET_DIR/release/kdj" /out/kdj
echo "built the deck for $codename: $(du -h /out/kdj | cut -f1)"
