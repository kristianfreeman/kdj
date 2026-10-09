#!/bin/bash
# Turn a fresh Raspberry Pi OS Lite (64-bit) on a Pi 4 into a deck: build
# it, then install.sh installs it, mounts USB sticks for it, and starts it
# full screen at boot.
#
# Run from the repository on the Pi, as your normal user (it uses sudo):
#
#   deploy/pi/setup.sh                  # everything, then reboot
#   deploy/pi/setup.sh --no-boot-config # leave config.txt / cmdline.txt alone
#   deploy/pi/setup.sh --build-only     # just build and install the binary
#
# Safe to run again (after a `git pull`, say): it rebuilds and reinstalls,
# keeps /etc/deck/deck.env, and replaces only its own block in config.txt.
#
# For an HDMI screen, set its mode: DECK_HDMI_MODE=800x480@60 deploy/pi/setup.sh
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
boot_config=1
build_only=0
for arg in "$@"; do
    case $arg in
    --no-boot-config) boot_config=0 ;;
    --build-only) build_only=1 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

say() { printf '\n== %s\n' "$*"; }

if [ "$(uname -m)" != aarch64 ]; then
    echo "This is for 64-bit Raspberry Pi OS (aarch64); this is $(uname -m)." >&2
    exit 1
fi
if [ "$(id -u)" -eq 0 ]; then
    echo "Run this as your normal user; it uses sudo where it needs to." >&2
    exit 1
fi

say "Packages"
# SDL2 (KMS/DRM display, touch), ALSA, and cmake + libclang for Ableton Link.
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
    build-essential pkg-config git curl ca-certificates \
    libsdl2-dev libasound2-dev cmake clang libclang-dev

say "Rust"
if ! command -v cargo >/dev/null && [ ! -x "$HOME/.cargo/bin/cargo" ]; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
# shellcheck source=/dev/null
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
rustup update stable >/dev/null 2>&1 || true

say "Build (10–20 minutes the first time on a Pi 4)"
(cd "$repo" && cargo build --release --locked -p deck)

say "Install"
# install.sh does the rest (shared with the SD card image build).
flags=()
[ "$build_only" -eq 1 ] && flags+=(--binary-only)
[ "$boot_config" -eq 0 ] && flags+=(--no-boot-config)
sudo env DECK_HDMI_MODE="${DECK_HDMI_MODE:-}" "$here/install.sh" "${flags[@]}" "$repo/target/release/deck"
[ "$build_only" -eq 1 ] && exit 0

say "Done"
cat <<'MSG'
Next:
  1. Check the DAC overlay and display lines in config.txt suit your hardware.
  2. sudo reboot — the deck starts full screen.
  3. Logs: journalctl -u deck -f   (look for "audio: realtime priority")
  4. Stop it to use the console: sudo systemctl stop deck
MSG
