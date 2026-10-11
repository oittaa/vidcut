#!/usr/bin/env bash
# Downloads the real-world test clips listed in tests/media-manifest.toml into
# $VIDCUT_TEST_DATA/media/<name>/ (default ~/Downloads/vidcut-tmp/media). Media
# never goes into git. Archives are extracted into a fresh empty directory and
# nothing downloaded is executed. Prints each file's sha256; a mismatch with
# the manifest is an error.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
root="${VIDCUT_TEST_DATA:-$HOME/Downloads/vidcut-tmp}/media"
mkdir -p "$root"

# name<TAB>file<TAB>url<TAB>sha256, read with Python's tomllib (isolated mode).
python3 -I -c '
import sys, tomllib
for c in tomllib.load(open(sys.argv[1], "rb"))["clip"]:
    print("\t".join([c["name"], c["file"], c["url"], c.get("sha256", "")]))
' "$repo/tests/media-manifest.toml" | while IFS=$'\t' read -r name file url sha; do
  dir="$root/$name"
  target="$dir/$file"
  if [[ -f "$target" ]]; then
    have="$(sha256sum "$target" | cut -d' ' -f1)"
    if [[ -z "$sha" || "$have" == "$sha" ]]; then
      echo "ok      $name ($have)"
      continue
    fi
    echo "stale   $name: sha256 $have, manifest says $sha; downloading again"
  fi
  mkdir -p "$dir"
  part="$dir/.download.part"
  echo "fetch   $name <- $url"
  if [[ -t 2 ]]; then progress=(--progress-bar); else progress=(-sS); fi
  curl -fL --retry 3 "${progress[@]}" -o "$part" "$url"
  if [[ "$url" == *.zip ]]; then
    unpack="$(mktemp -d "$dir/.unzip.XXXXXX")"
    unzip -q "$part" -d "$unpack"
    found="$(find "$unpack" -type f -name "$file" | head -n1)"
    [[ -n "$found" ]] || { echo "error: $file not in $url" >&2; exit 1; }
    mv "$found" "$target"
    rm -rf "$unpack" "$part"
  else
    mv "$part" "$target"
  fi
  have="$(sha256sum "$target" | cut -d' ' -f1)"
  if [[ -n "$sha" && "$have" != "$sha" ]]; then
    echo "error: $name sha256 $have does not match manifest $sha" >&2
    exit 1
  fi
  echo "done    $name ($have)"
done
