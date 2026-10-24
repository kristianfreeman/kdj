#!/bin/bash
# The enclosure in 3D, live: rebuilds the build123d model (real Pi 4 STEP,
# the screen, DAC and ribbon, the printed box) whenever hardware/enclosure/
# cad/*.py changes, checks every part against every other for collisions,
# and serves a viewer that picks the new parts up without losing the camera:
# http://localhost:8720/viewer.html
#
#   tools/enclosure-live.sh [PORT]
set -u
cd "$(dirname "$0")/../hardware/enclosure"
port=${1:-8720}
mkdir -p live
mark=$(mktemp)

[ -s parts/raspberry_pi_4_model_b.step ] || cad/fetch-parts.sh
docker image inspect kdj-cad >/dev/null 2>&1 || docker build -q -t kdj-cad cad >/dev/null

build() {
    echo "enclosure-live: building"
    docker run --rm -v "$PWD:/w" -w /w kdj-cad python cad/kdj.py 2>&1 | tail -n 3 | sed 's/^/  /'
}

touch "$mark"
build
python3 -m http.server "$port" --bind 127.0.0.1 >/dev/null 2>&1 &
server=$!
trap 'kill $server 2>/dev/null; rm -f "$mark"; exit 0' INT TERM
echo "enclosure-live: http://localhost:$port/viewer.html"
while sleep 0.5; do
    if [ -n "$(find cad -name '*.py' -newer "$mark" -print -quit)" ]; then
        touch "$mark"
        build
    fi
done
