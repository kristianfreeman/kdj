#!/bin/bash
# Lint and test on Linux arm64 (the Pi's architecture and OS family) in
# Docker: Debian 13 with exactly the packages deploy/pi/setup.sh installs.
# Catches Linux-only code paths (ALSA, realtime setup, GLES) on a Mac.
#
#   tools/linux-check.sh     (or: make linux)
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
docker volume create deck-linux-cache >/dev/null
exec docker run --rm --platform linux/arm64 \
    -v "$repo:/src:ro" -v deck-linux-cache:/cache --tmpfs /work:exec,size=2g \
    debian:trixie bash -euo pipefail -c '
export DEBIAN_FRONTEND=noninteractive CARGO_TARGET_DIR=/cache/target
apt-get update -qq
apt-get install -y -qq --no-install-recommends build-essential pkg-config curl ca-certificates \
    libsdl2-dev libasound2-dev cmake clang libclang-dev >/dev/null
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none >/dev/null 2>&1
. "$HOME/.cargo/env"
tar -C /src --exclude=./target --exclude=./.git -cf - . | tar -C /work -xf -
cd /work
rustc --version
cargo clippy -q --locked --workspace --all-targets -- -D warnings
cargo test -q --locked --workspace
cargo build -q --release --locked -p deck
echo "linux arm64: ok"
'
