# MusicOS task runner. `just --list` shows everything.

set shell := ["bash", "-uc"]

default:
    @just --list

# One-time environment bootstrap (toolchain, components, dev tools, git hooks)
setup:
    bash scripts/setup.sh

build:
    cargo build --workspace --all-targets

# Prefer nextest when installed; fall back to cargo test
test:
    @if command -v cargo-nextest >/dev/null 2>&1; then \
        cargo nextest run --workspace; \
    else \
        cargo test --workspace; \
    fi

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

lint: fmt-check check-layers
    cargo clippy --workspace --all-targets -- -D warnings

# Architecture dependency rule (docs/02 §2)
check-layers:
    python3 scripts/check_layers.py

audit:
    cargo audit
    cargo deny check

docs:
    cargo doc --workspace --no-deps

bench:
    cargo bench --workspace

run *ARGS:
    cargo run -p musicos-cli -- {{ARGS}}

clean:
    cargo clean

# Everything CI runs, locally
ci: lint test docs audit
