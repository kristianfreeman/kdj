#!/bin/bash
# The enclosure in 3D, live: re-exports its parts a second after any change
# to hardware/enclosure/*.scad and serves a viewer that picks them up without
# losing the camera: http://localhost:8720
#
#   tools/enclosure-live.sh [PORT] [MODEL]     (default 8720, slab.scad)
set -u
cd "$(dirname "$0")/../hardware/enclosure"
port=${1:-8720}
model=${2:-slab.scad}
parts=(base lid_placed ghost_pi ghost_screen)
image=openscad/openscad:dev
mkdir -p live
mark=$(mktemp)

export_all() {
    local t0 ok=1 p
    t0=$(date +%s)
    for p in "${parts[@]}"; do
        docker run --rm -v "$PWD:/w" -w /w "$image" openscad --backend=manifold -o "live/$p.stl" -D "part=\"$p\"" "$model" \
            2>&1 | grep -E 'ERROR|ECHO' | sed 's/^/  /' || true
        [ -s "live/$p.stl" ] || ok=0
    done
    date +%s%N >live/stamp.txt
    echo "enclosure-live: exported in $(( $(date +%s) - t0 )) s$([ $ok = 1 ] || echo ' (a part failed)')"
}

docker image inspect "$image" >/dev/null 2>&1 || docker pull -q "$image"
touch "$mark"
export_all
python3 -m http.server "$port" --bind 127.0.0.1 >/dev/null 2>&1 &
server=$!
trap 'kill $server 2>/dev/null; rm -f "$mark"; exit 0' INT TERM
echo "enclosure-live: http://localhost:$port/viewer.html"
while sleep 0.5; do
    if [ -n "$(find . -maxdepth 1 -name '*.scad' -newer "$mark" -print -quit)" ]; then
        touch "$mark"
        export_all
    fi
done
