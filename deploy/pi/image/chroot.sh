#!/bin/bash
# Runs inside the SD card image (chroot, see customize.sh): installs the
# runtime packages, then the deck with deploy/pi/install.sh, then checks
# what it can without a Pi. Staged at /tmp/kdj-install:
#   kdj                  the binary from build-binary.sh
#   deploy/pi/...        this directory's parent (install.sh and its files)
#   config/...           the example MIDI mapping
set -euo pipefail

stage=/tmp/kdj-install
export DEBIAN_FRONTEND=noninteractive LC_ALL=C.UTF-8
say() { printf '\n== %s\n' "$*"; }
fail() { echo "FAILED: $*" >&2; exit 1; }

say "Runtime packages"
# SDL2 (KMS/DRM and evdev input; it loads libgbm, libdrm, EGL and GLES at
# run time), Mesa's EGL/GLES with the V3D (Pi 4 3D) and VC4 (display)
# gallium drivers, ALSA (alsa-utils for aplay -l when checking the DAC),
# libstdc++ for Ableton Link, blkid/mountpoint for kdj-usb, sudo for the
# power-off button.
packages=(
    libsdl2-2.0-0 libgles2 libegl1 libegl-mesa0 libgbm1 libdrm2
    mesa-libgallium libgl1-mesa-dri
    libasound2t64 alsa-utils
    libstdc++6 util-linux sudo
)
apt-get update -qq
apt-get install -y -q --no-install-recommends "${packages[@]}"
apt-get clean

say "Install the deck"
"$stage/deploy/pi/install.sh" "$stage/kdj"

say "Hostname"
echo kdj >/etc/hostname
sed -i 's/^127\.0\.1\.1\s.*/127.0.1.1\t\tkdj/' /etc/hosts
grep -q '^127\.0\.1\.1' /etc/hosts || printf '127.0.1.1\t\tkdj\n' >>/etc/hosts
cat /etc/hostname

say "Check: libraries"
ldd /opt/kdj/bin/kdj
if ldd /opt/kdj/bin/kdj | grep -q 'not found'; then fail "unresolved libraries"; fi
# Loaded at run time (dlopen), so ldd can't see them.
lib=/usr/lib/aarch64-linux-gnu
for f in libSDL2-2.0.so.0 libEGL.so.1 libGLESv2.so.2 libgbm.so.1 libdrm.so.2 libudev.so.1 libasound.so.2; do
    [ -e "$lib/$f" ] || fail "missing $lib/$f"
done
ls "$lib"/libgallium-*.so >/dev/null 2>&1 || fail "no Mesa gallium library"
for d in v3d vc4; do
    [ -e "$lib/dri/${d}_dri.so" ] || fail "no Mesa $d driver"
done
ls -l "$lib"/libgallium-*.so "$lib/dri/v3d_dri.so" "$lib/dri/vc4_dri.so"

say "Check: the binary runs (every symbol bound, no display or sound card here)"
set +e
out=$(LD_BIND_NOW=1 /opt/kdj/bin/kdj --no-such-option 2>&1)
status=$?
set -e
echo "$out"
if [ "$status" -ne 1 ] || ! grep -q 'unknown option' <<<"$out"; then
    fail "kdj didn't start cleanly (exit $status)"
fi
# As the service runs it, with ALSA's null output standing in for the DAC:
# --device finds no DAC and falls back to the default (null) output, then
# on into SDL's KMS/DRM display, which isn't here.
home=$(mktemp -d)
printf 'pcm.!default { type null }\n' >"$home/.asoundrc"
chmod 755 "$home" && chmod 644 "$home/.asoundrc"
set +e
out=$(LD_BIND_NOW=1 SDL_VIDEODRIVER=kmsdrm timeout 20 runuser -u kdj -- \
    env HOME="$home" /opt/kdj/bin/kdj --synth --fullscreen --no-link --device hifiberry 2>&1)
status=$?
set -e
rm -rf "$home"
echo "$out"
echo "(exit $status: expected to fail cleanly without a display)"
if grep -qi 'symbol lookup error\|error while loading shared' <<<"$out"; then fail "missing symbols"; fi
grep -q '^kdj: ' <<<"$out" || fail "no clean error message"
grep -q '^audio: no output device matching "hifiberry", using the default' <<<"$out" ||
    fail "no fallback to the default output"
grep -q '^audio: .* Hz' <<<"$out" || fail "didn't open the (null) audio output"

say "Check: systemd units"
systemd-analyze verify --man=no kdj.service kdj-usb@sda1.service
systemctl is-enabled kdj.service
for u in getty@tty1.service autovt@tty1.service bluetooth.service apt-daily.timer \
    apt-daily-upgrade.timer man-db.timer e2scrub_all.timer dpkg-db-backup.timer \
    NetworkManager-wait-online.service ssh.service NetworkManager.service \
    wpa_supplicant.service userconfig.service rpi-resize.service cloud-init-main.service \
    regenerate_ssh_host_keys.service sshswitch.service; do
    printf '%-40s %s\n' "$u" "$(systemctl is-enabled "$u" 2>/dev/null || true)"
done
systemctl is-enabled kdj.service | grep -qx enabled || fail "kdj.service not enabled"
for u in getty@tty1.service bluetooth.service apt-daily.timer; do
    s=$(systemctl is-enabled "$u" 2>/dev/null || true)
    [ "$s" != enabled ] || fail "$u still enabled"
done
for u in getty@tty1.service autovt@tty1.service NetworkManager-wait-online.service userconfig.service; do
    [ "$(systemctl is-enabled "$u" 2>/dev/null || true)" = masked ] || fail "$u not masked"
done
cat /etc/systemd/system-preset/10-kdj.preset
for u in rpi-resize.service cloud-init-main.service; do
    systemctl is-enabled "$u" | grep -qx enabled || fail "first-boot unit $u not enabled"
done

say "Check: kdj user, sudoers, files"
id kdj
for g in audio video render input plugdev; do id -nG kdj | tr ' ' '\n' | grep -qx "$g" || fail "kdj not in $g"; done
ls -l /etc/sudoers.d/kdj
cat /etc/sudoers.d/kdj
visudo -c
[ "$(stat -c %a /etc/sudoers.d/kdj)" = 440 ] || fail "sudoers mode"
sudo -l -U kdj | tail -n 3
ls -la /opt/kdj/bin /etc/kdj /media/kdj /var/lib/kdj
cat /etc/kdj/kdj.env
grep -q '^KDJ_ARGS="--device hifiberry --buffer 128"$' /etc/kdj/kdj.env || fail "kdj.env"
cat /etc/systemd/system.conf.d/kdj.conf

say "Check: boot configuration"
echo "-- /boot/firmware/config.txt"
cat /boot/firmware/config.txt
echo "-- /boot/firmware/cmdline.txt"
cat /boot/firmware/cmdline.txt
cmd=" $(cat /boot/firmware/cmdline.txt) "
for o in console=tty3 root=PARTUUID= rootwait resize quiet loglevel=3 logo.nologo \
    vt.global_cursor_default=0 consoleblank=0 systemd.show_status=false udev.log_level=3; do
    case $cmd in *" $o"*) ;; *) fail "cmdline.txt lacks $o" ;; esac
done
case $cmd in *" console=tty1 "*) fail "console still on tty1" ;; esac
[ "$(wc -l </boot/firmware/cmdline.txt)" -eq 1 ] || fail "cmdline.txt is not one line"
for l in '^#dtoverlay=vc4-kms-v3d  # kdj' '^#dtparam=audio=on  # kdj' '^dtoverlay=vc4-kms-v3d,noaudio$' \
    '^dtparam=audio=off$' '^dtoverlay=hifiberry-dacplus$' '^dtoverlay=disable-bt$' '^disable_splash=1$'; do
    grep -q "$l" /boot/firmware/config.txt || fail "config.txt lacks $l"
done
# Imager's customisation and first boot need these untouched.
ls -l /boot/firmware/user-data /boot/firmware/meta-data /boot/firmware/network-config
echo "chroot checks: ok"
