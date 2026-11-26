#!/bin/bash
# CI's Rust: mise.toml's pinned version (the one pin), through rustup, which checks every part it downloads against the
# release's manifest. rustup itself, when the machine has none, is rustup-init at a pinned version, checked by its
# SHA-256 before it runs (not sh.rustup.rs's script, which fetches whatever is newest). CI's jobs run it
# (.forgejo/workflows/ci.yml):
#
#   scripts/ci_rust.sh [components] [targets]      e.g. scripts/ci_rust.sh rustfmt,clippy
#
# rustup and the toolchain go to $RUSTUP_HOME and $CARGO_HOME when they are set (scripts/linux/ci_setup.sh), else
# ~/.rustup and ~/.cargo. The PATH of cargo's bin/ is handed on to the later steps through $GITHUB_PATH.
set -euo pipefail
cd "$(dirname "$0")/.."
RUSTUP_VERSION=1.29.0
RUSTUP_SHA256=4acc9acc76d5079515b46346a485974457b5a79893cfb01112423c89aeb5aa10   # (x86_64-unknown-linux-gnu's rustup-init)
rust=$(sed -n 's/^rust = { version = "\([0-9.]*\)".*/\1/p' mise.toml)
[ -n "$rust" ] || { echo "ci_rust.sh: no rust version in mise.toml" >&2; exit 1; }
cargo_bin=${CARGO_HOME:-$HOME/.cargo}/bin
export PATH="$cargo_bin:$PATH"
if ! command -v rustup > /dev/null; then
    tmp=$(mktemp -d)
    curl -fsSL --proto '=https' --retry 3 -o "$tmp/rustup-init" \
        "https://static.rust-lang.org/rustup/archive/$RUSTUP_VERSION/x86_64-unknown-linux-gnu/rustup-init"
    echo "$RUSTUP_SHA256  $tmp/rustup-init" | sha256sum --check --strict
    chmod +x "$tmp/rustup-init"
    "$tmp/rustup-init" -y --profile minimal --default-toolchain none --no-modify-path
    rm -rf "$tmp"
fi
args=(--profile minimal)
[ -n "${1:-}" ] && args+=(--component "$1")
[ -n "${2:-}" ] && args+=(--target "$2")
rustup toolchain install "$rust" "${args[@]}"
rustup default "$rust"
if [ -n "${GITHUB_PATH:-}" ]; then
    echo "$cargo_bin" >> "$GITHUB_PATH"
elif [ -n "${BASH_ENV:-}" ]; then
    echo "export PATH=\"$cargo_bin:\$PATH\"" >> "$BASH_ENV"
fi
rustc --version
