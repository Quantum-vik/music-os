#!/usr/bin/env bash
# MusicOS development environment bootstrap (Linux/macOS; Windows: use WSL or
# install the same tools manually — see CONTRIBUTING.md).
set -euo pipefail
cd "$(dirname "$0")/.."

say()  { printf '\033[1;34m[setup]\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31m[setup]\033[0m %s\n' "$*" >&2; exit 1; }

# --- Rust toolchain -----------------------------------------------------------
command -v rustup >/dev/null 2>&1 || fail "rustup not found — install from https://rustup.rs then re-run"
say "rust toolchain: $(rustc --version 2>/dev/null || echo '(installing)')"
rustup toolchain install stable --profile default >/dev/null
rustup component add rustfmt clippy >/dev/null

# --- Cargo dev tools ----------------------------------------------------------
need_tool() { # name, crate
    if ! command -v "$1" >/dev/null 2>&1; then
        say "installing $2"
        cargo install --locked "$2"
    else
        say "$1 already installed"
    fi
}
need_tool cargo-nextest cargo-nextest
need_tool cargo-audit   cargo-audit
need_tool cargo-deny    cargo-deny
command -v just >/dev/null 2>&1 || say "NOTE: 'just' not found — install it (brew install just / cargo install just)"

# --- Python (AI tooling only) ---------------------------------------------------
if command -v python3 >/dev/null 2>&1; then
    say "python3: $(python3 --version)"
else
    say "NOTE: python3 not found — needed for scripts/check_layers.py and future AI tooling"
fi

# --- Node (desktop app, Phase 5) ------------------------------------------------
command -v node >/dev/null 2>&1 && say "node: $(node --version)" \
    || say "NOTE: node not found — only needed for the desktop app (Phase 5)"

# --- Git hooks ------------------------------------------------------------------
if [ -d .git ]; then
    git config core.hooksPath scripts/hooks
    chmod +x scripts/hooks/* 2>/dev/null || true
    say "git hooks path set to scripts/hooks"
else
    say "NOTE: not a git repository — skipping hook installation"
fi

say "done. try: just build && just test"
