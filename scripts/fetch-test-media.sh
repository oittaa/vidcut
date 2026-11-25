#!/usr/bin/env bash
# Downloads the real-world test clips listed in tests/media-manifest.toml into
# $VIDCUT_TEST_DATA/media/<name>/ (default ~/Downloads/vidcut-tmp/media). Media
# never goes into git. Archives are extracted into a fresh empty directory and
# nothing downloaded is executed. Prints each file's sha256; a mismatch with
# the manifest is an error. With names (`fetch-test-media.sh bbb-2160p60`), it
# gets only those clips.
#
# A clip with `dir` is a folder in its archive, for example the frames of a
# CinemaDNG clip: the script keeps that whole folder, and `file` is one file
# directly in it. A clip with `fetch = "browser"` comes only from a click in a
# web browser, because its server refuses curl: the script prints where to
# save the download, and unpacks and checks it on the next run.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
root="${VIDCUT_TEST_DATA:-$HOME/Downloads/vidcut-tmp}/media"
mkdir -p "$root"

# sha256sum where there is one; macOS has shasum.
sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

# The manifest's clips, one line each, the fields name file url sha256 dir
# fetch page split by the unit separator \x1f: each [[clip]] and its
# key = "text" lines, all the manifest uses.
clips() {
  local name="" file="" url="" sha="" dir="" fetch="" page="" line pair='^([a-z0-9_]+) = "(.*)"$'
  emit() {
    if [[ -n "$name" ]]; then
      printf '%s\x1f%s\x1f%s\x1f%s\x1f%s\x1f%s\x1f%s\n' "$name" "$file" "$url" "$sha" "$dir" "$fetch" "$page"
    fi
  }
  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line%$'\r'}"
    if [[ "$line" == "[[clip]]" ]]; then
      emit
      name="" file="" url="" sha="" dir="" fetch="" page=""
    elif [[ "$line" =~ $pair ]]; then
      case "${BASH_REMATCH[1]}" in
        name) name="${BASH_REMATCH[2]}" ;;
        file) file="${BASH_REMATCH[2]}" ;;
        url) url="${BASH_REMATCH[2]}" ;;
        sha256) sha="${BASH_REMATCH[2]}" ;;
        dir) dir="${BASH_REMATCH[2]}" ;;
        fetch) fetch="${BASH_REMATCH[2]}" ;;
        page) page="${BASH_REMATCH[2]}" ;;
      esac
    fi
  done <"$repo/tests/media-manifest.toml"
  emit
}

manual=0
while IFS=$'\x1f' read -r name file url sha keep fetch page; do
  if [[ $# -gt 0 && " $* " != *" $name "* ]]; then continue; fi
  dir="$root/$name"
  if [[ -n "$keep" ]]; then target="$dir/$keep/$file"; else target="$dir/$file"; fi
  if [[ -f "$target" ]]; then
    have="$(sha256 "$target")"
    if [[ -z "$sha" || "$have" == "$sha" ]]; then
      echo "ok      $name ($have)"
      continue
    fi
    if [[ "$fetch" == "browser" ]]; then
      echo "stale   $name: sha256 $have, manifest says $sha; download it again from $page"
      manual=$((manual + 1))
      continue
    fi
    echo "stale   $name: sha256 $have, manifest says $sha; downloading again"
  fi
  mkdir -p "$dir"
  if [[ "$fetch" == "browser" ]]; then
    if [[ "$url" == *.zip ]]; then part="$dir/${url##*/}"; else part="$target"; fi
    if [[ ! -f "$part" ]]; then
      echo "browser $name: in a web browser, open $page, download $url and save it as $part"
      manual=$((manual + 1))
      continue
    fi
    echo "unpack  $name <- $part"
  else
    part="$dir/.download.part"
    echo "fetch   $name <- $url"
    if [[ -t 2 ]]; then progress=(--progress-bar); else progress=(-sS); fi
    curl -fL --retry 3 "${progress[@]}" -o "$part" "$url"
  fi
  if [[ "$url" == *.zip ]]; then
    unpack="$(mktemp -d "$dir/.unzip.XXXXXX")"
    unzip -q "$part" -d "$unpack"
    found="$(find "$unpack" -type f -name "$file" | head -n1)"
    [[ -n "$found" ]] || { echo "error: $file not in $url" >&2; exit 1; }
    if [[ -n "$keep" ]]; then
      src="$(dirname "$found")"
      [[ "$(basename "$src")" == "$keep" ]] || { echo "error: $file is not in a folder $keep in $url" >&2; exit 1; }
      rm -rf "${dir:?}/$keep"
      mv "$src" "$dir/$keep"
    else
      mv "$found" "$target"
    fi
    rm -rf "$unpack" "$part"
  elif [[ "$part" != "$target" ]]; then
    mv "$part" "$target"
  fi
  have="$(sha256 "$target")"
  if [[ -n "$sha" && "$have" != "$sha" ]]; then
    echo "error: $name sha256 $have does not match manifest $sha" >&2
    exit 1
  fi
  echo "done    $name ($have)"
done < <(clips)
if [[ $manual -gt 0 ]]; then
  echo "$manual clip(s) need a download in a web browser; see the browser and stale lines above"
fi
