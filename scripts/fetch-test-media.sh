#!/usr/bin/env bash
# Downloads the real-world test clips listed in tests/media-manifest.toml into
# $VIDCUT_TEST_DATA/media/<name>/ (default ~/Downloads/vidcut-tmp/media). Media
# never goes into git. Archives are extracted into a fresh empty directory and
# nothing downloaded is executed. Prints each file's sha256; a mismatch with
# the manifest is an error. With names (`fetch-test-media.sh bbb-2160p60`), it
# gets only those clips.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
root="${VIDCUT_TEST_DATA:-$HOME/Downloads/vidcut-tmp}/media"
mkdir -p "$root"

# sha256sum where there is one; macOS has shasum.
sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

# The manifest's clips, name<TAB>file<TAB>url<TAB>sha256: each [[clip]] and
# its key = "text" lines, all the manifest uses.
clips() {
  local name="" file="" url="" sha="" line pair='^([a-z0-9_]+) = "(.*)"$'
  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line%$'\r'}"
    if [[ "$line" == "[[clip]]" ]]; then
      if [[ -n "$name" ]]; then printf '%s\t%s\t%s\t%s\n' "$name" "$file" "$url" "$sha"; fi
      name="" file="" url="" sha=""
    elif [[ "$line" =~ $pair ]]; then
      case "${BASH_REMATCH[1]}" in
        name) name="${BASH_REMATCH[2]}" ;;
        file) file="${BASH_REMATCH[2]}" ;;
        url) url="${BASH_REMATCH[2]}" ;;
        sha256) sha="${BASH_REMATCH[2]}" ;;
      esac
    fi
  done <"$repo/tests/media-manifest.toml"
  if [[ -n "$name" ]]; then printf '%s\t%s\t%s\t%s\n' "$name" "$file" "$url" "$sha"; fi
}

clips | while IFS=$'\t' read -r name file url sha; do
  if [[ $# -gt 0 && " $* " != *" $name "* ]]; then continue; fi
  dir="$root/$name"
  target="$dir/$file"
  if [[ -f "$target" ]]; then
    have="$(sha256 "$target")"
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
  have="$(sha256 "$target")"
  if [[ -n "$sha" && "$have" != "$sha" ]]; then
    echo "error: $name sha256 $have does not match manifest $sha" >&2
    exit 1
  fi
  echo "done    $name ($have)"
done
