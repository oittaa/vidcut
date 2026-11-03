#!/usr/bin/env bash
# Asks the export and the viewer about the same clips, with the upscaler of macOS.
# In each case, the two answers must agree.
# Run it from the repo. Paste what it prints.

set -u
cargo build --release -q -p vidcut || exit 1
vidcut=${VIDCUT:-target/release/vidcut}
dir=$(mktemp -d)

clip() {
    cargo run --release -q -p vidcut-av --example make_fixture -- h264 mp4 "$1" 30 15 no-audio 2>/dev/null
}

answer() {
    local said
    if said=$("$@" 2>&1); then echo ok; else echo "$said" | tail -1; fi
}

try() {
    local project="$dir/$1.vidcut"
    export VIDCUT_PROJECT="$project"
    "$vidcut" project create "$project" --fps 30 --width "${3%x*}" --height "${3#*x}" >/dev/null || exit 1
    "$vidcut" media add "$2" >/dev/null || exit 1
    "$vidcut" edit append m1 --range 0f..15f >/dev/null || exit 1
    "$vidcut" media set m1 --super-scale true --upscaler macos >/dev/null || exit 1
    echo "== $1: $(basename "$2" | cut -d_ -f3) clip in a $3 timeline"
    echo "export: $(answer "$vidcut" export start "$dir/$1.mp4")"
    echo "viewer: $(answer "$vidcut" frame render --at 5f --out "$dir/$1.png" --viewer)"
}

big=$(clip 3840x2160)
small=$(clip 1920x1080)
try fits-as-it-is "$big" 3840x2160
try too-large "$big" 5760x3240
try enlarges "$small" 3840x2160
