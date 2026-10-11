#!/bin/bash
# The deck on this computer, as on the Pi (an 800x480 window, the Xone:K3
# map, your Rekordbox collection and any USB sticks), rebuilt and restarted
# a couple of seconds after any change to the code. Ctrl-C or closing the
# window stops it.
#
#   tools/dev.sh [kdj options...]    e.g. --decks 2, --synth, a track's path
set -u
cd "$(dirname "$0")/.."

args=(--midi config/xone-k3.toml "$@")
mark=$(mktemp)
pid=
stop() {
    [ -z "$pid" ] && return
    kill "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
    pid=
}
trap 'stop; rm -f "$mark"; exit 0' INT TERM

run() {
    echo "dev: building"
    if ! cargo build -q --release -p kdj; then
        echo "dev: the build failed; $( [ -n "$pid" ] && echo "the last one keeps running" || echo "fix it and save")"
        return
    fi
    stop
    target/release/kdj "${args[@]}" &
    pid=$!
    echo "dev: running (pid $pid). Change any code to rebuild and restart."
}

touch "$mark"
run
while sleep 0.5; do
    if [ -n "$(find crates config assets -newer "$mark" -type f \( -name '*.rs' -o -name '*.toml' -o -name '*.otf' -o -name '*.ttf' \) -print -quit)" ]; then
        touch "$mark"
        run
    elif [ -n "$pid" ] && ! kill -0 "$pid" 2>/dev/null; then
        # The window was closed (or the deck stopped): so do we.
        wait "$pid" 2>/dev/null
        echo "dev: the deck stopped (exit $?)"
        rm -f "$mark"
        exit 0
    fi
done
