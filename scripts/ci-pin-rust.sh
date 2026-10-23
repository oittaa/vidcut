#!/usr/bin/env bash
# Verify the pinned Rust toolchain and remove every other rustup toolchain.
# Swatinem/rust-cache hashes all installed rustc builds into its env key.
# GitHub-hosted runners sometimes keep an older one from the image; that
# flips the key and makes PRs miss the master cache. Call this after
# checkout and before Swatinem/rust-cache.
set -euo pipefail
cd "$(dirname "$0")/.."

channel=$(awk -F'"' '/^channel/ { print $2; exit }' rust-toolchain.toml)
if [[ -z "$channel" ]]; then
  echo "no channel in rust-toolchain.toml" >&2
  exit 1
fi

rustup show active-toolchain
rustc --version
got=$(rustc --version | awk '{ print $2 }')
if [[ "$got" != "$channel" ]]; then
  echo "expected rustc ${channel} from rust-toolchain.toml, got ${got}" >&2
  exit 1
fi

while IFS= read -r tc; do
  case "$tc" in
    "$channel" | "$channel"-*) continue ;;
  esac
  echo "uninstalling extra toolchain: ${tc}"
  rustup toolchain uninstall "$tc"
done < <(rustup toolchain list | awk '{ print $1 }')

# The image may have marked the removed toolchain as default.
rustup default "$channel"
rustup toolchain list
