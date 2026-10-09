#!/bin/bash
# Runs in a privileged Debian arm64 container (see build.sh). Two steps:
#
#   customize.sh prepare BASE.img.xz   decompress to /out/work/kdj.img and
#                                      print the image's Debian codename
#   customize.sh build NAME            grow it, install the deck in a chroot
#                                      (chroot.sh), shrink it, and write
#                                      /out/NAME.img.xz and NAME.img.xz.sha256
#
# Mounts: /src the repository (read-only), /out target/image (with the
# binary at /out/build/kdj), /cache the downloaded image.
set -euo pipefail

img=/out/work/kdj.img
root=/mnt/kdj
loop=
say() { printf '\n== %s\n' "$*" >&2; }

tools() {
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends fdisk e2fsprogs zerofree xz-utils mount util-linux \
        systemd-container >/dev/null
}

# losetup -P, plus the partition nodes: the kernel makes them, but a
# container's /dev doesn't get them by itself.
attach() {
    # Left over from an interrupted run.
    losetup -j "$img" | cut -d: -f1 | xargs -r -n1 losetup -d
    loop=$(losetup --find --show --partscan "$@" "$img")
    local n=${loop#/dev/} p
    for p in "/sys/block/$n/$n"p*; do
        [ -e "/dev/${p##*/}" ] || mknod "/dev/${p##*/}" b "$(cut -d: -f1 "$p/dev")" "$(cut -d: -f2 "$p/dev")"
    done
    [ -b "${loop}p2" ] || { echo "no partitions on $loop" >&2; exit 1; }
}

detach() {
    [ -n "$loop" ] || return 0
    losetup -d "$loop" 2>/dev/null || true
    rm -f "$loop"p[0-9]*
    loop=
}

unmount_all() {
    mountpoint -q "$root" || return 0
    # Anything apt left running in the chroot.
    local p
    for p in /proc/[0-9]*; do
        case $(readlink "$p/root" 2>/dev/null) in
        "$root"*) kill -9 "${p#/proc/}" 2>/dev/null || true ;;
        esac
    done
    umount -R "$root" 2>/dev/null || umount -R -l "$root"
}

cleanup() {
    unmount_all || true
    detach
}
trap cleanup EXIT
trap 'exit 143' TERM INT

prepare() {
    local xz=$1
    tools
    say "Decompressing $(basename "$xz")"
    mkdir -p /out/work
    rm -f "$img"
    xz -dc -T0 "$xz" >"$img"
    attach --read-only
    mkdir -p "$root"
    mount -o ro "${loop}p2" "$root"
    # shellcheck source=/dev/null
    (. "$root/etc/os-release" && echo "image: $PRETTY_NAME" >&2 && echo "$VERSION_CODENAME")
    umount "$root"
    detach
}

# What this build is, as key=value lines (kdj-image.txt on the card, and
# the .info file next to the image, which adds the uncompressed size).
info() {
    echo "# The deck's SD card image (deploy/pi/image). kdj-flash status reads this."
    echo "image=$name"
    echo "built=$built"
    echo "kdj=${KDJ_VERSION:-unknown}"
    echo "commit=${KDJ_COMMIT:-unknown}"
    echo "base=${KDJ_BASE:-unknown}"
}

build() {
    local name=$1 grow=${KDJ_IMAGE_GROW:-1536M}
    built=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    tools
    [ -f "$img" ] || { echo "run prepare first" >&2; exit 1; }
    [ -x /out/build/kdj ] || { echo "no /out/build/kdj" >&2; exit 1; }

    say "Growing the root partition by $grow"
    truncate -s "+$grow" "$img"
    echo ', +' | sfdisk --quiet --no-reread --no-tell-kernel -N 2 "$img"
    attach
    e2fsck -pf "${loop}p2"
    resize2fs "${loop}p2"

    say "Mounting"
    mkdir -p "$root"
    mount "${loop}p2" "$root"
    mount "${loop}p1" "$root/boot/firmware"
    for d in dev proc sys; do
        mount --rbind "/$d" "$root/$d"
        mount --make-rslave "$root/$d"
    done
    # What first boot relies on: these must come out of the chroot unchanged.
    local before
    before=$(cd "$root" && ls -l etc/machine-id etc/ssh/ssh_host_* 2>/dev/null; cat etc/machine-id 2>/dev/null)

    # Name resolution for apt, and no services started by package scripts.
    cp -a "$root/etc/resolv.conf" /tmp/resolv.conf.image
    cp /etc/resolv.conf "$root/etc/resolv.conf"
    printf '#!/bin/sh\nexit 101\n' >"$root/usr/sbin/policy-rc.d"
    chmod 755 "$root/usr/sbin/policy-rc.d"

    # Stage the binary and deploy/pi (install.sh and its files).
    local stage=$root/tmp/kdj-install
    rm -rf "$stage"
    mkdir -p "$stage/deploy" "$stage/config"
    cp -a /src/deploy/pi "$stage/deploy/pi"
    cp -a /src/config/midi-example.toml "$stage/config/"
    install -m 755 /out/build/kdj "$stage/kdj"

    say "Installing in the chroot"
    chroot "$root" /tmp/kdj-install/deploy/pi/image/chroot.sh

    rm -rf "$stage" "$root/usr/sbin/policy-rc.d"
    cp -a /tmp/resolv.conf.image "$root/etc/resolv.conf"
    # Which build this is, on the boot partition (readable from any
    # computer), for kdj-flash status.
    info >"$root/boot/firmware/kdj-image.txt"
    local after
    after=$(cd "$root" && ls -l etc/machine-id etc/ssh/ssh_host_* 2>/dev/null; cat etc/machine-id 2>/dev/null)
    if [ "$before" != "$after" ]; then
        echo "machine-id or SSH host keys changed in the chroot; first boot needs them as shipped" >&2
        exit 1
    fi
    df -h "$root" "$root/boot/firmware" >&2
    unmount_all

    say "Shrinking"
    local p2=${loop}p2 bs min blocks start sectors mib=$((1024 * 1024))
    e2fsck -fy "$p2" || [ $? -le 1 ]
    bs=$(dumpe2fs -h "$p2" 2>/dev/null | awk -F: '/^Block size/ {gsub(/ /, "", $2); print $2}')
    min=$(resize2fs -P "$p2" 2>/dev/null | awk -F': ' '/minimum size/ {print $2}')
    # Room to spare until first boot grows it to fill the card.
    blocks=$((min + 256 * mib / bs))
    resize2fs "$p2" "$blocks"
    zerofree "$p2"
    e2fsck -fn "$p2"
    detach
    start=$(partx -g -o START -n 2 "$img" | tr -d ' ')
    # The filesystem's size in sectors, rounded up to 4 MiB.
    sectors=$(((blocks * bs + 4 * mib - 1) / (4 * mib) * (4 * mib) / 512))
    echo "$start,$sectors" | sfdisk --quiet --no-reread --no-tell-kernel -N 2 "$img"
    truncate -s $(((start + sectors) * 512)) "$img"
    sfdisk -d "$img" >&2

    say "Checking the result"
    attach --read-only
    e2fsck -fn "${loop}p2" >&2
    mkdir -p "$root"
    mount -o ro "${loop}p1" "$root"
    local id
    id=$(sfdisk --disk-id "$img" | sed 's/^0x//')
    grep -q "root=PARTUUID=$id-02 " "$root/cmdline.txt" ||
        { echo "cmdline.txt root= doesn't match the disk id $id" >&2; exit 1; }
    echo "disk id $id matches cmdline.txt" >&2
    umount "$root"
    detach

    boot_test

    say "Compressing"
    xz -T0 -6 -c "$img" >"/out/$name.img.xz.part"
    mv "/out/$name.img.xz.part" "/out/$name.img.xz"
    (cd /out && sha256sum "$name.img.xz" >"$name.img.xz.sha256" && cat "$name.img.xz.sha256")
    { info; echo "size=$(stat -c %s "$img")"; } >"/out/$name.img.xz.info"
    echo "uncompressed: $(du -h --apparent-size "$img" | cut -f1)" >&2
    rm -f "$img"
}

# Boot the finished image (its own systemd, as on a first boot, in a
# container with no display or sound card), and check from inside it: the
# presets first boot applies, the deck service restarting after it fails
# for want of a sound card, the sudoers rule. The image itself isn't
# touched: the container writes to a throwaway overlay.
boot_test() {
    say "Booting it in a container (systemd-nspawn, first boot)"
    attach --read-only
    mkdir -p "$root"
    mount -o ro "${loop}p2" "$root"
    local probe=/tmp/kdj-probe log=/tmp/kdj-probe/out/log
    rm -rf "$probe"
    mkdir -p "$probe/out"
    cat >"$probe/kdj-probe.service" <<'UNIT'
[Unit]
Description=kdj image boot test
After=multi-user.target kdj.service
[Service]
Type=oneshot
StandardOutput=append:/kdj-probe-out/log
StandardError=inherit
ExecStart=/bin/sh /usr/local/sbin/kdj-probe
UNIT
    cat >"$probe/kdj-probe" <<'PROBE'
#!/bin/sh
sleep 12
for u in kdj.service getty@tty1.service autovt@tty1.service bluetooth.service \
    apt-daily.timer apt-daily-upgrade.timer man-db.timer e2scrub_all.timer \
    dpkg-db-backup.timer NetworkManager-wait-online.service \
    systemd-networkd-wait-online.service NetworkManager.service; do
    echo "probe: is-enabled $u $(systemctl is-enabled "$u" 2>&1)"
done
echo "probe: getty@tty1 $(systemctl is-active getty@tty1.service)"
echo "probe: kdj $(systemctl show kdj.service -p ActiveState -p SubState -p NRestarts | tr '\n' ' ')"
journalctl -b -u kdj.service -o cat --no-pager | grep -v '^ALSA lib' | tail -n 4 | sed 's/^/probe: journal: /'
echo "probe: sudo $(sudo -n -l -U kdj 2>/dev/null | tail -n 1)"
echo "probe: done"
systemctl poweroff
PROBE
    timeout --signal=KILL 180 systemd-nspawn --quiet --register=no --keep-unit \
        --directory="$root" --volatile=overlay --boot \
        --bind-ro="$probe/kdj-probe.service:/etc/systemd/system/kdj-probe.service" \
        --bind-ro="$probe/kdj-probe:/usr/local/sbin/kdj-probe" \
        --bind="$probe/out:/kdj-probe-out" \
        systemd.wants=kdj-probe.service </dev/null >"$probe/console" 2>&1 || true
    umount "$root"
    detach
    grep '^probe: ' "$log" >&2 || { cat "$probe/console" >&2; echo "the boot test didn't finish" >&2; exit 1; }
    local bad=0 u
    grep -q '^probe: done' "$log" || bad=1
    grep -q '^probe: is-enabled kdj.service enabled$' "$log" || bad=1
    for u in getty@tty1 bluetooth apt-daily apt-daily-upgrade man-db e2scrub_all dpkg-db-backup \
        NetworkManager-wait-online systemd-networkd-wait-online; do
        if grep -q "^probe: is-enabled $u\.[a-z]* enabled$" "$log"; then bad=1; echo "$u enabled after first boot" >&2; fi
    done
    grep -q '^probe: is-enabled autovt@tty1.service masked$' "$log" || bad=1
    grep -q '^probe: getty@tty1 active$' "$log" && bad=1
    # Restarted (it has no sound card here) and still going: no start limit.
    grep -Eq '^probe: kdj .*NRestarts=([3-9]|[1-9][0-9])' "$log" || bad=1
    grep -q '^probe: kdj .*ActiveState=failed' "$log" && bad=1
    grep -q 'NOPASSWD: /usr/bin/systemctl poweroff, /usr/bin/systemctl reboot' "$log" || bad=1
    [ "$bad" -eq 0 ] || { echo "boot test failed" >&2; exit 1; }
    echo "boot test: ok" >&2
}

case ${1:-} in
prepare) prepare "$2" ;;
build) build "$2" ;;
*) echo "usage: customize.sh prepare BASE.img.xz | build NAME" >&2; exit 2 ;;
esac
