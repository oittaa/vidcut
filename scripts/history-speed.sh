#!/usr/bin/env bash
# How long edits and loads take on a big project, and how large its undo
# history grows. It makes a project of 300 media and 2000 clips, fills the
# history with 100 steps, then times EDITS edits (a trim, a ripple delete
# and a marker in turn), each followed by a command that only reads.
#
#   scripts/history-speed.sh [EDITS]
#
# Needs release builds: `cargo build --release -p vidcut` and
# `cargo build --release -p vidcut-av --example make_fixture`. Everything it
# writes, the history too, goes to a temporary folder. KEEP=1 keeps it.
set -euo pipefail

edits=${1:-50}
bin="$(cd "$(dirname "$0")/.." && pwd)/target/release"
exe() { if [ -x "$1" ]; then echo "$1"; else echo "$1.exe"; fi; }
vidcut=$(exe "$bin/vidcut")
make_fixture=$(exe "$bin/examples/make_fixture")

work=$(mktemp -d)
[ "${KEEP:-}" = 1 ] || trap 'rm -rf "$work"' EXIT
export XDG_STATE_HOME="$work/state"
export VIDCUT_PROJECT="$work/big.vidcut"
run() { "$vidcut" --headless "$@" >/dev/null; }
now() { date +%s%N; }
history_bytes() { cat "$XDG_STATE_HOME"/vidcut/history/* 2>/dev/null | wc -c; }

echo "making the project in $work"
clip=$("$make_fixture" h264 mp4 320x180 25 250)
mkdir "$work/media"
for i in $(seq -w 1 300); do cp "$clip" "$work/media/clip$i.mp4"; done
run project create "$VIDCUT_PROJECT" --fps 25 --width 1920 --height 1080
run media add "$work"/media/*.mp4
for i in $(seq 0 1999); do
    printf '{"method":"edit.append","params":{"media":"m%d","range":"0f..50f"}}\n' $((i % 300 + 1))
done >"$work/clips.jsonl"
run apply "$work/clips.jsonl"
echo "filling the history"
for i in $(seq 1 100); do run markers add --at "${i}s" --name "fill $i"; done

t0=$(now)
"$vidcut" --version >/dev/null
start_ms=$((($(now) - t0) / 1000000))
echo "project $(wc -c <"$VIDCUT_PROJECT") bytes, history $(history_bytes) bytes, starting vidcut ${start_ms} ms"
printf 'edit\tedit ms\tread ms\thistory bytes\n'
for i in $(seq 1 "$edits"); do
    case $((i % 3)) in
        1) kind=trim cmd=(edit trim "c$((100 + i))" --edge tail --by=-2f) ;;
        2) kind=delete cmd=(edit delete "V1@$((1000 + 10 * i))s") ;;
        0) kind=marker cmd=(markers add --at "$((200 + i))s" --name "m$i") ;;
    esac
    t0=$(now)
    run "${cmd[@]}"
    t1=$(now)
    run project summary
    t2=$(now)
    printf '%s\t%d\t%d\t%d\n' "$kind" $(((t1 - t0) / 1000000)) $(((t2 - t1) / 1000000)) "$(history_bytes)"
done
