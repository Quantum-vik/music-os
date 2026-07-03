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

## AI: your subscription or your API key — your choice

`music ai "<brief>"` runs a production agent over the open project. Two
providers, selected with `--provider` (default `auto`):

| Provider | How it reasons | Auth |
|---|---|---|
| `subscription` | Spawns your local Claude Code headless, wired to MusicOS's MCP server as its only tool source | Your existing Claude subscription — no API keys |
| `api` | In-process tool loop against the Anthropic Messages API (default model `claude-opus-4-8`, `--model` to change) | `ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN` |

```sh
music ai "add a keys track, import idea.mid, pan it slightly left, render a draft"
music ai "make the project 92 BPM" --provider api --model claude-sonnet-5
```

`auto` picks subscription when the `claude` CLI is installed, else api when
credentials are present. Both providers use the same deterministic tool
registry: every AI action is validated, logged with an agent actor, and
undoable. Token efficiency is designed in — strict schemas, terse
descriptions, summary-first outputs, prompt-cache breakpoints on the stable
prefix, and budget caps on the loop (docs/06).

## Use with Claude (your subscription, no API keys)

MusicOS ships an MCP server. Claude Code spawns it locally and drives every
MusicOS tool — composition, project editing, mixing, rendering — using **your
existing Claude subscription** for the reasoning. Nothing is sent anywhere
except your normal Claude conversation; MusicOS executes locally and
deterministically, and every AI action lands in the project's undoable
command log with actor `agent:mcp`.

```sh
cargo install --path apps/server        # or: cargo build --release
claude mcp add musicos -- music-server --project /path/to/Song.musicos
```

Then, in Claude Code:

> "Create a project called Sunrise, add a keys track, import idea.mid,
> pan the keys slightly left, drop the tempo to 92, and render a draft."

Token efficiency is designed in: strict input schemas, terse tool
descriptions, compact `{data, summary}` outputs, and `get_project_summary`
digests instead of raw project files (docs/06 §5, docs/07 §3).

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

## Use MusicOS inside your DAW (FL Studio, Bitwig, Reaper, ...)

MusicOS ships a CLAP plugin, **MusicOS Player** (`plugins/player`), that
plays a `.musicos` project inside any plugin host, synced to the host
transport:

1. Build it: `cargo build --release -p musicos-player`
2. Copy/rename the built library to your CLAP folder, e.g. on macOS:
   `cp target/release/libmusicos_player.dylib ~/Library/Audio/Plug-Ins/CLAP/"MusicOS Player.clap"`
3. Point it at a project: `export MUSICOS_PROJECT=/path/to/Song.musicos`
   (or write the path to `~/.musicos/player-project.txt`), then start your DAW
   from that shell and add "MusicOS Player" as an instrument.

For hosts without CLAP support (e.g. current FL Studio), wrap the `.clap`
into a VST3 with the MIT-licensed [clap-wrapper](https://github.com/free-audio/clap-wrapper)
— the standard route used by Surge and friends; no VST3 SDK license enters
this repository.

### Live MIDI into your DAW's own synths

`music stream` plays the project as **live MIDI** through a virtual port —
FL Studio's (or any DAW's) own instruments render the music in real time:

1. macOS/Linux: just run `music stream` — a "MusicOS Out" port appears
   (macOS: enable Audio MIDI Setup -> IAC driver if you prefer routing there).
   Windows: create a port with loopMIDI, then `music stream --port loopMIDI`.
2. In the DAW, set that port as the MIDI input for your instruments.
   Each MusicOS track streams on its own MIDI channel (track 0 -> ch 1, ...).
3. `music stream --from-bar 16` seeks; Ctrl-C stops (with all-notes-off).

### Tempo-sync with Ableton Link

`music-link` (separate GPL-2.0 binary — Ableton Link's license) joins the
local Link session and streams the project as MIDI at the **session tempo**,
launching quantized to the bar and following live tempo changes:

```
cargo build --release -p musicos-link
./target/release/music-link Song.musicos            # virtual "MusicOS Link Out" port
./target/release/music-link Song.musicos --port loopMIDI --quantum 4
```

Enable Link in your DAW (Live, FL Studio, Bitwig), point its MIDI input at
the port, press play anywhere — everything locks to the same beat grid.
