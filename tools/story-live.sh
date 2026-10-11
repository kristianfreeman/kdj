#!/bin/bash
# Every screen (the storybook), redrawn a few seconds after any change to the
# code, on a page that reloads itself: http://localhost:8710
#
#   tools/story-live.sh [PORT]
set -u
cd "$(dirname "$0")/.."
port=${1:-8710}
out=target/storybook
mark=$(mktemp)
export KDJ_STORY_LIVE=1

draw() { cargo run -q --release -p kdj-sim -- story "$out" || echo "story-live: the build failed; fix it and save"; }

mkdir -p "$out"
touch "$mark"
draw
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$out" >/dev/null 2>&1 &
server=$!
trap 'kill $server 2>/dev/null; rm -f "$mark"; exit 0' INT TERM
echo "story-live: http://localhost:$port"
while sleep 0.5; do
    if [ -n "$(find crates config assets -newer "$mark" -type f \( -name '*.rs' -o -name '*.toml' -o -name '*.otf' -o -name '*.ttf' \) -print -quit)" ]; then
        touch "$mark"
        draw
    fi
done
