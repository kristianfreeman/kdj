#!/bin/bash
# Build a Raspberry Pi 4 SD card image with the deck installed, booting
# straight into it: the latest Raspberry Pi OS Lite (64-bit), plus the deck
# built for that release, set up by deploy/pi/install.sh. Flash it with
# Raspberry Pi Imager ("Use custom"), setting the user, SSH and Wi-Fi in its
# OS customisation.
#
#   deploy/pi/image/build.sh        (or: make image)
#
# Needs Docker with linux/arm64 containers (native on Apple Silicon; under
# emulation elsewhere, much slower) and runs one privileged container for the
# loop mounts. Writes target/image/kdjos-rpi4-<date>.img.xz and its .sha256.
# Downloads are cached in target/image/cache; the cargo cache lives in the
# Docker volume kdj-image-cargo.
#
# To build on a particular release instead of the latest, give its URL:
#   RASPIOS_URL=https://downloads.raspberrypi.com/raspios_lite_arm64/images/raspios_lite_arm64-2026-10-06/2026-10-06-raspios-trixie-arm64-lite.img.xz
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
out=$repo/target/image
cache=$out/cache
name=kdjos-rpi4-$(date +%Y-%m-%d)
containers=(kdj-image-prepare kdj-image-binary kdj-image-build)
say() { printf '\n== %s\n' "$*"; }

# One build at a time: builds share the containers' names and the work
# image, so a second would kill the first's containers and overwrite its
# image. Checked before the cleanup below is set, which would stop them.
lock=$out/.building
mkdir -p "$out"
if ! mkdir "$lock" 2>/dev/null; then
    pid=$(cat "$lock/pid" 2>/dev/null || true)
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        echo "Another image build is running (pid $pid): wait for it, it'll leave the image in $out." >&2
        exit 1
    fi
    rm -rf "$lock" # left by a build that died
    mkdir "$lock"
fi
echo $$ >"$lock/pid"

cleanup() {
    # Stopped, not killed: the image step unmounts and detaches on SIGTERM.
    for c in "${containers[@]}"; do docker stop -t 60 "$c" >/dev/null 2>&1 || true; done
    rm -rf "$lock"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# docker run, removed afterwards (or on Ctrl-C).
run() {
    local c=$1
    shift
    docker rm -f "$c" >/dev/null 2>&1 || true
    docker run --rm --init -e TINI_KILL_PROCESS_GROUP=1 --name "$c" --platform linux/arm64 "$@"
}

start=$(date +%s)
mkdir -p "$cache" "$out/build" "$out/work"

say "Raspberry Pi OS Lite (64-bit)"
url=${RASPIOS_URL:-$(curl -fsSIL -o /dev/null -w '%{url_effective}' https://downloads.raspberrypi.com/raspios_lite_arm64_latest)}
case $url in
https://downloads.raspberrypi.com/*.img.xz) ;;
*) echo "unexpected image URL: $url" >&2; exit 1 ;;
esac
xz=$(basename "$url")
echo "$url"
# The checksum Raspberry Pi publishes next to the image, fetched every time.
curl -fsSL -o "$cache/$xz.sha256" "$url.sha256"
if [ ! -f "$cache/$xz" ]; then
    curl -fL --progress-bar -o "$cache/$xz.part" "$url"
    mv "$cache/$xz.part" "$cache/$xz"
fi
(cd "$cache" && if command -v sha256sum >/dev/null; then sha256sum -c "$xz.sha256"; else shasum -a 256 -c "$xz.sha256"; fi) || {
    echo "checksum mismatch: deleting $cache/$xz, run again" >&2
    rm -f "$cache/$xz"
    exit 1
}

say "Which Debian release it is"
codename=$(run kdj-image-prepare --privileged \
    -v "$repo:/src:ro" -v "$out:/out" -v "$cache:/cache:ro" \
    debian:trixie /src/deploy/pi/image/customize.sh prepare "/cache/$xz" | tail -n1)
case $codename in
'' | *[!a-z]*) echo "couldn't read the image's release ($codename)" >&2; exit 1 ;;
esac
echo "Debian $codename"

say "Building the deck on Debian $codename (arm64)"
docker volume create kdj-image-cargo >/dev/null
run kdj-image-binary \
    -v "$repo:/src:ro" -v kdj-image-cargo:/cache -v "$out/build:/out" \
    --tmpfs /work:exec,size=2g \
    "debian:$codename" /src/deploy/pi/image/build-binary.sh

say "Installing into the image"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/crates/kdj/Cargo.toml" | head -n1)
commit=$(git -C "$repo" describe --always --dirty 2>/dev/null || echo unknown)
run kdj-image-build --privileged --cgroupns=host \
    -e "KDJ_VERSION=$version" -e "KDJ_COMMIT=$commit" -e "KDJ_BASE=${xz%.img.xz}" \
    -v "$repo:/src:ro" -v "$out:/out" \
    "debian:$codename" /src/deploy/pi/image/customize.sh build "$name"

rm -rf "$out/work"
say "Done in $(( ($(date +%s) - start) / 60 )) min $(( ($(date +%s) - start) % 60 )) s"
ls -lh "$out/$name.img.xz"
cat "$out/$name.img.xz.sha256"
echo "Based on $xz. Flash with Raspberry Pi Imager: Use custom → $out/$name.img.xz"
