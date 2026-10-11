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

step "rustfmt"
cargo fmt --all --check

step "clippy (warnings are errors)"
cargo clippy --workspace --all-targets --quiet "$@" -- -D warnings

case "$mode" in
  fast)
    step "unit tests"
    cargo test --workspace --lib --bins --quiet "$@"
    ;;
  full)
    step "all tests (unit, integration, e2e, doc)"
    cargo test --workspace --all-targets --quiet "$@"
    cargo test --workspace --doc --quiet "$@"
    ;;
  *)
    echo "usage: $0 [fast|full] [cargo flags...]" >&2
    exit 2
    ;;
esac

printf '\n\033[1;32mcheck (%s): OK\033[0m\n' "$mode"
