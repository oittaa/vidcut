#!/usr/bin/env bash
# Quality gates for vidcut. See AGENTS.md ("Quality gates").
#
#   scripts/check.sh          fast: fmt + clippy + unit tests (run before every commit)
#   scripts/check.sh full     fast + integration/e2e + doc tests (after bigger changes, per milestone)
#   scripts/check.sh full --features gpu-tests,media-tests
#                             also GPU encoder and real-media tests
set -euo pipefail
cd "$(dirname "$0")/.."

mode="${1:-fast}"
shift || true

step() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }

# --quiet hides "Compiling …" until the command exits. Locally that keeps the
# pre-commit hook short. GitHub Actions sets CI=true; there the log is the
# only progress, so print Cargo's normal status lines.
cargo_quiet=(--quiet)
if [[ -n "${CI:-}" ]]; then
  cargo_quiet=()
fi

step "no binary files"
# Media and test data live in $VIDCUT_TEST_DATA, never in git (AGENTS.md).
# Git's own test decides what is binary: `--numstat` shows "-" for it. The
# index is what a commit would hold (in CI, the checkout).
empty_tree=$(git hash-object -t tree /dev/null)
binary=$(git diff --cached --numstat "$empty_tree" | awk -F'\t' '$1 == "-" { print $3 }')
if [[ -n "$binary" ]]; then
  echo "binary files can't go in git (test data belongs in \$VIDCUT_TEST_DATA):" >&2
  printf '%s\n' "$binary" | sed 's/^/  /' >&2
  exit 1
fi

step "rustfmt"
cargo fmt --all --check

step "clippy (warnings are errors)"
cargo clippy --workspace --all-targets "${cargo_quiet[@]}" "$@" -- -D warnings

case "$mode" in
  fast)
    step "unit tests"
    cargo test --workspace --lib --bins "${cargo_quiet[@]}" "$@"
    ;;
  full)
    step "all tests (unit, integration, e2e, doc)"
    cargo test --workspace --all-targets "${cargo_quiet[@]}" "$@"
    cargo test --workspace --doc "${cargo_quiet[@]}" "$@"
    ;;
  *)
    echo "usage: $0 [fast|full] [cargo flags...]" >&2
    exit 2
    ;;
esac

printf '\n\033[1;32mcheck (%s): OK\033[0m\n' "$mode"
