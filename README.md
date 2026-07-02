# MusicOS

**An operating system for autonomous music production — not a DAW.**

MusicOS is a set of reusable Rust services that expose every music-production
capability (composition, arrangement, MIDI, mixing, rendering) as APIs and tools.
Humans, AI agents (via [MCP](docs/07_MCP_Architecture.md)), CLIs, desktop apps, and
external systems all drive the exact same core. The CLI is just one client. The GUI
is just another client.

> **Status: Phase 0 — foundations.** Architecture is fully specified in
> [`docs/`](docs/README.md); implementation follows the
> [development roadmap](docs/12_Development_Roadmap.md). Nothing here makes sound yet.

## Design documentation

Start with the [vision](docs/00_Vision.md), then the
[docs index](docs/README.md) (14 design documents + [ADR log](docs/adr/README.md)).

## Development

```sh
just setup     # bootstrap toolchain, dev tools, git hooks
just build
just test
just lint      # fmt check + clippy -D warnings + architecture layer check
just ci        # everything CI runs
just run init MySong
```

Requirements: stable Rust (via rustup), `just`, python3. Node is only needed for the
desktop app (Phase 5).

## Layout

```
apps/        clients: cli, server (MCP), desktop (Phase 5)
crates/      the core: domain, services, engine, adapters (see docs/02 §3)
sdk/         stable façades (Rust now, Python later)
plugins/     first-party plugins against plugin-api
docs/        design docs + ADRs — the source of truth
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Significant decisions require an
[ADR](docs/adr/template.md) before implementation.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
