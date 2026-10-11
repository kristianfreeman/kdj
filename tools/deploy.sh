#!/bin/bash
# Put this checkout's deck on kdj Pis that are already running, over SSH:
# the program and the controller mappings, then restart the deck. Seconds,
# not a reflash: most changes are to the program. (Changes to the system
# set-up in deploy/pi still need `make image` and a flash.)
#
#   tools/deploy.sh [HOST...]   default: every kdj announcing itself here
#
# Logs in as $KDJ_USER (default: you), with the key kdj-flash put on the card.
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
user=${KDJ_USER:-$USER}

hosts=("$@")
if [ ${#hosts[@]} -eq 0 ]; then
    echo "== Looking for kdjs on the network"
    while read -r ip name role; do
        [ -n "$ip" ] || continue
        case $role in library*) echo "   $name ($ip): a library, not a deck; skipped"; continue ;; esac
        echo "   $name ($ip): $role"
        hosts+=("$ip")
    done < <(python3 "$repo/tools/find-kdjs.py" 3)
    if [ ${#hosts[@]} -eq 0 ]; then
        echo "No kdj heard on this network. Give its name: tools/deploy.sh kdj-oak-pike.local" >&2
        exit 1
    fi
fi

echo "== Building the deck for the Pi (arm64)"
out=$repo/target/deploy
mkdir -p "$out"
docker volume create kdj-image-cargo >/dev/null
docker build -q --platform linux/arm64 --build-arg CODENAME=trixie -t kdj-image-tools:trixie \
    - <"$repo/deploy/pi/image/tools.Dockerfile" >/dev/null
docker run --rm --init --platform linux/arm64 \
    -v "$repo:/src:ro" -v kdj-image-cargo:/cache -v "$out:/out" --tmpfs /work:exec,size=2g \
    kdj-image-tools:trixie /src/deploy/pi/image/build-binary.sh | tail -n 1
commit=$(git -C "$repo" describe --always --dirty 2>/dev/null || echo unknown)

failed=0
for host in "${hosts[@]}"; do
    echo "== $host"
    if ! scp -q -o ConnectTimeout=8 "$out/kdj" "$repo"/config/*.toml "$user@$host:/tmp/" ||
        ! ssh -o ConnectTimeout=8 "$user@$host" "set -e
            sudo install -m 755 /tmp/kdj /opt/kdj/bin/kdj
            for m in /tmp/*.toml; do sudo install -m 644 \"\$m\" /etc/kdj/; done
            rm -f /tmp/kdj /tmp/*.toml
            sudo systemctl restart kdj
            sleep 2
            systemctl is-active --quiet kdj"; then
        echo "   failed" >&2
        failed=1
        continue
    fi
    echo "   running $commit"
done
exit $failed
