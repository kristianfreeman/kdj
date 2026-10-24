#!/bin/bash
# Install a built deck onto Raspberry Pi OS and make the Pi an appliance
# that boots straight into it. Shared by setup.sh (on the Pi, after it
# builds) and image/build.sh (in a chroot of the SD card image). Run as root:
#
#   install.sh [--no-boot-config] [--binary-only] PATH/TO/kdj
#
#   --binary-only     just replace /opt/kdj/bin/kdj and kdj-usb
#   --no-boot-config  leave config.txt and cmdline.txt alone
#
# Safe to run again: it keeps /etc/kdj/kdj.env and replaces only its own
# block in config.txt. On a running system it also reloads systemd and udev;
# in a chroot (no systemd running) it only enables and disables units.
#
# For an HDMI screen, set its mode: KDJ_HDMI_MODE=800x480@60
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
boot_config=1
binary_only=0
binary=
for arg in "$@"; do
    case $arg in
    --no-boot-config) boot_config=0 ;;
    --binary-only) binary_only=1 ;;
    -*) echo "unknown option: $arg" >&2; exit 2 ;;
    *) binary=$arg ;;
    esac
done
if [ -z "$binary" ] || [ ! -f "$binary" ]; then
    echo "usage: install.sh [--no-boot-config] [--binary-only] PATH/TO/kdj" >&2
    exit 2
fi
if [ "$(id -u)" -ne 0 ]; then
    echo "install.sh needs root." >&2
    exit 1
fi
# A booted system (not a chroot): reload and start/stop things now.
live=0
[ -d /run/systemd/system ] && live=1

install -d /opt/kdj/bin /opt/kdj/share /etc/kdj /media/kdj
install -m 755 "$binary" /opt/kdj/bin/kdj
install -m 755 "$here/kdj-usb" "$here/kdj-name" "$here/kdj-dac" /opt/kdj/bin/
install -m 644 "$here/names.txt" /opt/kdj/share/names.txt
if [ "$binary_only" -eq 1 ]; then
    echo "Installed /opt/kdj/bin/kdj."
    exit 0
fi

# The deck's own system user, with the screen, sound, input and USB sticks.
if ! id kdj >/dev/null 2>&1; then
    useradd --system --create-home --home-dir /var/lib/kdj --shell /usr/sbin/nologin kdj
fi
for g in audio video render input plugdev; do
    if getent group "$g" >/dev/null; then usermod -aG "$g" kdj; fi
done
[ -f /etc/kdj/kdj.env ] || install -m 644 "$here/kdj.env" /etc/kdj/kdj.env
# The Xone:K3 map is ours, so it's updated; the example is only a starting point.
install -m 644 "$repo/config/xone-k3.toml" /etc/kdj/xone-k3.toml
if [ -f "$repo/config/midi-example.toml" ] && [ ! -f /etc/kdj/midi-example.toml ]; then
    install -m 644 "$repo/config/midi-example.toml" /etc/kdj/midi-example.toml
fi

# Power off, restart, and Wi-Fi on and off from the deck's screen: exactly
# these commands.
sudoers=$(mktemp)
printf '%s\n' 'kdj ALL=(root) NOPASSWD: /usr/bin/systemctl poweroff, /usr/bin/systemctl reboot, /usr/bin/nmcli radio wifi on, /usr/bin/nmcli radio wifi off' >"$sudoers"
chmod 440 "$sudoers"
visudo -cqf "$sudoers"
install -m 440 "$sudoers" /etc/sudoers.d/kdj
rm -f "$sudoers"

install -m 644 "$here/kdj.service" "$here/kdj-usb@.service" "$here/kdj-name.service" "$here/kdj-dac.service" \
    /etc/systemd/system/
install -m 644 "$here/99-kdj-usb.rules" /etc/udev/rules.d/
# A cable straight to another kdj: link-local addresses when there's no DHCP.
install -d /etc/NetworkManager/conf.d
install -m 644 "$here/nm-kdj.conf" /etc/NetworkManager/conf.d/kdj.conf
# No boot status on any screen, whatever happens to cmdline.txt.
install -d /etc/systemd/system.conf.d
printf '[Manager]\nShowStatus=no\n' >/etc/systemd/system.conf.d/kdj.conf
if [ "$live" -eq 1 ]; then
    systemctl daemon-reload
    udevadm control --reload
fi

# The deck owns tty1: no login prompt there, at boot or on a VT switch.
# Masked, not just disabled: Raspberry Pi OS's user scripts (cancel-rename,
# rename-user) enable getty@tty1 again, and then it, not the deck, may win
# the screen at boot.
systemctl disable getty@tty1.service >/dev/null 2>&1 || true
systemctl mask getty@tty1.service >/dev/null
systemctl mask autovt@tty1.service >/dev/null
systemctl enable kdj.service kdj-name.service kdj-dac.service

# Nothing in the background mid-set or slowing boot: package and man-page
# housekeeping, modems, hotkeys, Bluetooth. Wi-Fi and SSH stay as they are.
quiet=(apt-daily.timer apt-daily-upgrade.timer man-db.timer e2scrub_all.timer
    dpkg-db-backup.timer ModemManager.service triggerhappy.service
    triggerhappy.socket bluetooth.service hciuart.service)
# Nor waiting for the network at boot (Link finds it when it comes up).
# Masked: enabling NetworkManager or systemd-networkd, as first boot does,
# would enable these again.
wait_online=(NetworkManager-wait-online.service systemd-networkd-wait-online.service)
# Never a keyboard: Raspberry Pi OS's first-boot "create a user" dialog is
# off for good. A login comes from cloud-init (kdj-flash or Imager writes
# user-data on the card) or not at all; the deck runs as its own user.
never_ask=(userconfig.service)
now=()
[ "$live" -eq 1 ] && now=(--now)
exists() { systemctl list-unit-files --no-legend "$1" 2>/dev/null | grep -q .; }
for unit in "${quiet[@]}"; do
    if exists "$unit"; then systemctl disable "${now[@]}" "$unit" >/dev/null 2>&1 || true; fi
done
for unit in "${wait_online[@]}" "${never_ask[@]}"; do
    if exists "$unit"; then systemctl mask "${now[@]}" "$unit" >/dev/null 2>&1 || true; fi
done
# On a fresh image's first boot systemd applies the presets to every unit,
# and Debian's enable almost everything: these lines win over them.
install -d /etc/systemd/system-preset
{
    echo "# kdj (deploy/pi/install.sh): what first boot or preset-all must keep."
    echo "enable kdj.service"
    echo "enable kdj-name.service"
    echo "enable kdj-dac.service"
    echo "disable getty@.service"
    printf 'disable %s\n' "${quiet[@]}"
} >/etc/systemd/system-preset/10-kdj.preset

if [ "$boot_config" -eq 1 ]; then
    fw=/boot/firmware
    [ -d "$fw" ] || fw=/boot
    config=$fw/config.txt
    cmdline=$fw/cmdline.txt
    [ -f "$config.kdj-backup" ] || cp "$config" "$config.kdj-backup"
    [ -f "$cmdline.kdj-backup" ] || cp "$cmdline" "$cmdline.kdj-backup"
    # Our block replaces the stock graphics and audio lines.
    sed -i \
        -e '/^# --- kdj: added/,/^# --- kdj: end ---/d' \
        -e 's/^dtoverlay=vc4-kms-v3d$/#&  # kdj: see below/' \
        -e 's/^dtparam=audio=on$/#&  # kdj: see below/' \
        "$config"
    # The block opens with [all], whatever section the file ends in.
    cat "$here/config.txt" >>"$config"
    # cmdline.txt is one line. Boot messages go to tty3, out of sight; add
    # the quiet-boot options it doesn't have yet. root= and the rest stay.
    opts="quiet loglevel=3 logo.nologo vt.global_cursor_default=0 consoleblank=0"
    opts="$opts systemd.show_status=false udev.log_level=3"
    [ -n "${KDJ_HDMI_MODE:-}" ] && opts="$opts video=HDMI-A-1:$KDJ_HDMI_MODE"
    line=" $(head -n1 "$cmdline") "
    line=${line// console=tty1 / console=tty3 }
    for o in $opts; do
        key=${o%%=*}
        case $line in
        *" $key="* | *" $o "*) ;;
        *) line="$line$o " ;;
        esac
    done
    line=${line# }
    printf '%s\n' "${line% }" >"$cmdline"
    echo "Boot configuration: $config, $cmdline (backups: *.kdj-backup)"
fi
