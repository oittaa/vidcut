#!/bin/bash
# macOS: makes H.264 and ProRes Proxy proxies, one at a time, and says for
# each whether its frames left the GPU. macOS's `sample` watches the encode:
# frames copied down to memory show as av_hwframe_transfer_data, pictures
# scaled on the CPU as libswscale.
#   scripts/proxy-gpu-check.sh [FILE...]
# Without files: 10 s of 4K60 HEVC and of AV1, barcode fixtures made once in
# $VIDCUT_TEST_DATA.
set -euo pipefail
sources=("$@")
cd "$(dirname "$0")/.."

cargo build -q --release -p vidcut -p vidcut-av --example make_fixture
bin=target/release
if [ ${#sources[@]} -eq 0 ]; then
    echo "making the 4K sources (once)..."
    sources=(
        "$("$bin/examples/make_fixture" hevc10 mkv 3840x2160 60 600 no-audio)"
        "$("$bin/examples/make_fixture" av1 mkv 3840x2160 60 600 no-audio)"
    )
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" XDG_STATE_HOME="$tmp/state"

echo "vidcut $(git rev-parse --short HEAD), $(sysctl -n machdep.cpu.brand_string), macOS $(sw_vers -productVersion)"
for path in "${sources[@]}"; do
    source=$(basename "$path")
    for codec in h264 prores; do
        dir="$tmp/$source-$codec"
        mkdir -p "$dir"
        "$bin/vidcut" project create "$dir/p.vidcut" >/dev/null
        "$bin/vidcut" -p "$dir/p.vidcut" media add "$path" >/dev/null
        "$bin/vidcut" preferences set "proxies.codec=$codec" >/dev/null
        /usr/bin/time -p "$bin/vidcut" -p "$dir/p.vidcut" proxy generate --json \
            >"$dir/out.json" 2>"$dir/time.txt" &
        timer=$!
        pid=""
        for _ in $(seq 50); do
            pid=$(pgrep -P "$timer" || true)
            [ -n "$pid" ] && break
            sleep 0.05
        done
        # Past opening the file and the decoder.
        sleep 0.5
        if [ -n "$pid" ]; then
            sample "$pid" 30 -mayDie -file "$dir/sample.txt" >/dev/null 2>&1 || true
        fi
        if ! wait "$timer"; then
            echo "$source -> $codec failed:"
            tail -5 "$dir/time.txt"
            continue
        fi
        real=$(awk '/^real/ {print $2}' "$dir/time.txt")
        cpu=$(awk '/^(user|sys)/ {s += $2} END {print s}' "$dir/time.txt")
        made=$(grep -o '"made_with": "[^"]*"' "$dir/out.json" | cut -d'"' -f4)
        down=no
        scaled=no
        if [ -f "$dir/sample.txt" ]; then
            grep -q 'av_hwframe_transfer_data' "$dir/sample.txt" && down=yes
            grep -q '(in libswscale' "$dir/sample.txt" && scaled=yes
        else
            down="not sampled"
            scaled="not sampled"
        fi
        echo "$source -> $codec: $real s, CPU $cpu s, copied down: $down, scaled on the CPU: $scaled ($made)"
    done
done
