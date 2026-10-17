#!/usr/bin/env bash
# Checks the docs against the repository:
#   - every backticked name in docs/design/ exists in crates/
#   - every relative link in a Markdown file points at a file that exists,
#     and its #anchor at a heading of that file
set -euo pipefail
cd "$(dirname "$0")/.."
export LC_ALL=C

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

comm -23 <(git ls-files -co --exclude-standard | sort -u) <(git ls-files -d | sort -u) > "$tmp/files"
awk -F/ '{ p = ""; for (i = 1; i < NF; i++) { p = p (i > 1 ? "/" : "") $i; print p } print }' \
  "$tmp/files" | sort -u > "$tmp/paths"
grep '\.md$' "$tmp/files" > "$tmp/docs" || true

grep -E '^crates/.*\.(rs|wgsl|toml)$' "$tmp/files" | while IFS= read -r f; do cat "$f"; done \
  | grep -oE '[A-Za-z_][A-Za-z0-9_]*|[A-Za-z0-9_][A-Za-z0-9_.-]*' | sort -u > "$tmp/tokens"

# GitHub's heading anchors: lowercase, punctuation dropped, spaces to hyphens,
# repeats numbered.
xargs awk '
  FNR == 1 { fence = 0; split("", seen) }
  /^ *(```|~~~)/ { fence = !fence; next }
  fence { next }
  /^#+ / {
    s = $0; sub(/^#+ +/, "", s); sub(/ +#+ *$/, "", s)
    s = tolower(s); gsub(/[^a-z0-9 _-]/, "", s); gsub(/ /, "-", s)
    if (s in seen) { n = seen[s]; seen[s] = n + 1; s = s "-" n } else seen[s] = 1
    print FILENAME "#" s
  }
' < "$tmp/docs" > "$tmp/anchors"

status=0

xargs awk -v paths="$tmp/paths" -v anchors="$tmp/anchors" '
  function norm(p,   n, parts, out, i, k, r) {
    n = split(p, parts, "/"); k = 0
    for (i = 1; i <= n; i++) {
      if (parts[i] == "" || parts[i] == ".") continue
      if (parts[i] == "..") { if (k == 0) return "../"; k-- } else out[++k] = parts[i]
    }
    r = ""
    for (i = 1; i <= k; i++) r = r (i > 1 ? "/" : "") out[i]
    return r
  }
  BEGIN {
    while ((getline l < paths) > 0) path[l] = 1
    while ((getline l < anchors) > 0) anchor[l] = 1
  }
  FNR == 1 { fence = 0; dir = FILENAME; if (!sub(/\/[^\/]*$/, "", dir)) dir = "" }
  /^ *(```|~~~)/ { fence = !fence; next }
  fence { next }
  {
    line = $0
    gsub(/`[^`]*`/, "", line)
    while (match(line, /\]\([^)]*\)/)) {
      t = substr(line, RSTART + 2, RLENGTH - 3)
      line = substr(line, RSTART + RLENGTH)
      sub(/ +".*"$/, "", t)
      if (t ~ /^[a-z]+:/) continue
      a = ""
      if (index(t, "#")) { a = tolower(substr(t, index(t, "#") + 1)); t = substr(t, 1, index(t, "#") - 1) }
      f = (t == "") ? FILENAME : norm((dir == "" ? "" : dir "/") t)
      if (!(f in path)) { printf "%s:%d: link to a missing file: %s\n", FILENAME, FNR, t; bad = 1; continue }
      if (a != "" && f ~ /\.md$/ && !((f "#" a) in anchor)) {
        printf "%s:%d: link to a missing heading: %s#%s\n", FILENAME, FNR, t, a; bad = 1
      }
    }
  }
  END { exit bad }
' < "$tmp/docs" || status=1

if compgen -G 'docs/design/*.md' > /dev/null; then
  awk -v tokens="$tmp/tokens" -v paths="$tmp/paths" '
    function known(s,   n, segs, i, g, h) {
      n = split(s, segs, "::")
      for (i = 1; i <= n; i++) {
        g = segs[i]
        if (g == "" || g in tok) continue
        h = g; sub(/[0-9.N]+$/, "", h)
        if (h != "" && h in tok) continue
        return 0
      }
      return 1
    }
    BEGIN {
      while ((getline l < tokens) > 0) tok[l] = 1
      while ((getline l < paths) > 0) path[l] = 1
    }
    FNR == 1 { fence = 0 }
    /^ *(```|~~~)/ { fence = !fence; next }
    fence { next }
    {
      line = $0
      while (match(line, /`[^`]+`/)) {
        span = substr(line, RSTART + 1, RLENGTH - 2)
        line = substr(line, RSTART + RLENGTH)
        if (span ~ / / || span ~ /^[-~$%]/) continue
        s = span
        gsub(/<[^>]*>/, "", s); gsub(/\{[^}]*\}/, "", s)
        sub(/\(\)$/, "", s); sub(/\.\*$/, "", s); sub(/^\./, "", s)
        if (s ~ /\//) {
          sub(/\/$/, "", s)
          if (!(s in path)) { printf "%s:%d: no such path: %s\n", FILENAME, FNR, span; bad = 1 }
        } else if (!known(s)) {
          printf "%s:%d: not found in crates/: %s\n", FILENAME, FNR, span; bad = 1
        }
      }
    }
    END { exit bad }
  ' docs/design/*.md || status=1
fi

exit "$status"
