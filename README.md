# MusicOS

**An operating system for autonomous music production — not a DAW.**

MusicOS is a set of reusable Rust services that expose every music-production
capability (composition, arrangement, MIDI, mixing, rendering) as APIs and tools.
Humans, AI agents (via [MCP](docs/07_MCP_Architecture.md)), CLIs, desktop apps, and
external systems all drive the exact same core. The CLI is just one client. The GUI
is just another client.

## What This Repo Is About In Music Production

Think of MusicOS as a **programmable backend for music production**: a system that
treats music-making tasks as structured tools and services instead of hiding all logic
inside a traditional DAW interface.

Most DAWs are built around a human clicking through a GUI. MusicOS is trying to build
the layer underneath that GUI so the same production capabilities can be used from a
CLI, an AI agent, an MCP client, a desktop app, or a batch pipeline. The idea is that
composition, arrangement, project editing, mixing, rendering, and playback should all
be accessible through one shared core.

### What Problems It Is Trying To Solve

This repo is meant to help with the gap between:

- **creative intent**: "make this song more cinematic", "write a darker chorus", "render stems"
- **actual execution**: editing notes, changing arrangement, updating project state,
  applying tools, saving results, and rendering output reproducibly

In other words, MusicOS is not only about generating ideas. It is about turning music
production tasks into deterministic operations that software can inspect, execute, undo,
replay, and automate.

### What It Covers In A Music Production Workflow

The long-term goal is to support most of the production pipeline through reusable tools:

- **Composition**: generate and edit melodies, chord progressions, basslines, drum patterns,
  motifs, and harmonic movement
- **Arrangement**: shape song sections such as intro, verse, chorus, bridge, drop, outro,
  and transition moments
- **Project editing**: manage tracks, clips, tempo maps, time signatures, command history,
  undo/redo, and persistent project state
- **MIDI workflows**: import/export MIDI, transpose, quantize, humanize, and transform
  symbolic performance data
- **Mixing and sound shaping**: eventually control levels, buses, sends, dynamics, EQ,
  space, stereo image, and mastering-style processing
- **Rendering and delivery**: eventually render final mixes, stems, and machine-friendly
  outputs in headless workflows
- **AI-assisted production**: let LLMs or other agents plan work, inspect project state,
  call tools, and apply changes through the same deterministic system humans use

### What Makes It Different

MusicOS is not mainly:

- a chat wrapper that depends mostly on prompt output
- a DAW clone where the GUI is the product
- a macro layer on top of another workstation

Instead, it is trying to become the **shared execution layer** underneath music-production
clients such as a CLI, MCP server, desktop app, and SDKs. AI is intended to act as a
planner or operator, while the core services perform the real work.

### Example Workflows This Repo Is Aiming For

- "Create a new song from a short brief and render a draft"
- "Import a MIDI idea, quantize it, humanize it, and place it in a project"
- "Make the chorus bigger, more cinematic, and more punchy"
- "Let an AI agent inspect a project, propose arrangement and mix changes, apply them, and render the result"
- "Batch render hundreds of projects headlessly in CI with structured JSON output"

### What Exists Today

Today the repo is still early-stage and focused on the symbolic/project foundation:

- a Rust workspace with a documented architecture and staged roadmap
- a working CLI surface
- a project model with commands, events, and undo/redo
- open bundle-style project storage
- MIDI import/export and transformation utilities
- a canonical tool registry intended to be shared by CLI and future MCP/AI surfaces

The audio engine, real-time playback, plugin hosting, full AI runtime, and production-grade
rendering pipeline are planned later phases of the project.

> **Status: early Phase 2.** Architecture is fully specified in
> [`docs/`](docs/README.md); implementation follows the
> [development roadmap](docs/12_Development_Roadmap.md). The symbolic core,
> project model with undo, bundle storage, tool registry, and a first offline
> WAV renderer (`music render`) are working; the real-time engine, mixer graph,
> plugin hosting, and AI runtime are still ahead.

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
