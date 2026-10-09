#!/bin/bash
# Turn a fresh Raspberry Pi OS Lite (64-bit) on a Pi 4 into a deck: build
# it, install it, mount USB sticks for it, and start it full screen at boot.
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
sudo install -d /opt/deck/bin /etc/deck /media/deck
sudo install -m 755 "$repo/target/release/deck" /opt/deck/bin/deck
sudo install -m 755 "$here/deck-usb" /opt/deck/bin/deck-usb
if [ "$build_only" -eq 1 ]; then
    echo "Installed /opt/deck/bin/deck."
    exit 0
fi

if ! id deck >/dev/null 2>&1; then
    sudo useradd --system --create-home --home-dir /var/lib/deck --shell /usr/sbin/nologin deck
fi
for g in audio video render input plugdev; do
    if getent group "$g" >/dev/null; then sudo usermod -aG "$g" deck; fi
done
[ -f /etc/deck/deck.env ] || sudo install -m 644 "$here/deck.env" /etc/deck/deck.env
if [ -f "$repo/config/midi-example.toml" ] && [ ! -f /etc/deck/midi-example.toml ]; then
    sudo install -m 644 "$repo/config/midi-example.toml" /etc/deck/midi-example.toml
fi
sudo install -m 644 "$here/deck.service" "$here/deck-usb@.service" /etc/systemd/system/
sudo install -m 644 "$here/99-deck-usb.rules" /etc/udev/rules.d/
sudo systemctl daemon-reload
sudo udevadm control --reload
sudo systemctl disable getty@tty1.service >/dev/null 2>&1 || true
sudo systemctl enable deck.service

if [ "$boot_config" -eq 1 ]; then
    say "Boot configuration"
    fw=/boot/firmware
    [ -d "$fw" ] || fw=/boot
    config=$fw/config.txt
    cmdline=$fw/cmdline.txt
    [ -f "$config.deck-backup" ] || sudo cp "$config" "$config.deck-backup"
    [ -f "$cmdline.deck-backup" ] || sudo cp "$cmdline" "$cmdline.deck-backup"
    # Our block replaces the stock graphics and audio lines.
    sudo sed -i \
        -e '/^# --- deck: added/,/^# --- deck: end ---/d' \
        -e 's/^dtoverlay=vc4-kms-v3d$/#&  # deck: see below/' \
        -e 's/^dtparam=audio=on$/#&  # deck: see below/' \
        "$config"
    # The block opens with [all], whatever section the file ends in.
    # shellcheck disable=SC2024 # we read it; sudo tee writes
    sudo tee -a "$config" <"$here/config.txt" >/dev/null
    # cmdline.txt is one line: add quiet-boot options it doesn't have yet.
    opts="quiet loglevel=3 logo.nologo vt.global_cursor_default=0 consoleblank=0"
    [ -n "${DECK_HDMI_MODE:-}" ] && opts="$opts video=HDMI-A-1:$DECK_HDMI_MODE"
    line=$(head -n1 "$cmdline")
    for o in $opts; do
        key=${o%%=*}
        case " $line " in
        *" $key="* | *" $o "*) ;;
        *) line="$line $o" ;;
        esac
    done
    printf '%s\n' "$line" | sudo tee "$cmdline" >/dev/null
    echo "Backups: $config.deck-backup, $cmdline.deck-backup"
fi

say "Done"
cat <<'MSG'
Next:
  1. Check the DAC overlay and display lines in config.txt suit your hardware.
  2. sudo reboot — the deck starts full screen.
  3. Logs: journalctl -u deck -f   (look for "audio: realtime priority")
  4. Stop it to use the console: sudo systemctl stop deck
MSG
