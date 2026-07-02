# 02 — System Architecture

Status: Draft v1 · Depends on: `00_Vision.md`, `01_Product_Requirements.md`
Key ADRs: ADR-0001 (hexagonal), ADR-0002 (workspace layout), ADR-0003 (CQRS + event log)

## 1. Architectural Style

**Hexagonal (ports & adapters) core, CQRS-shaped application layer, actor-style engine,
modular monolith deployment.**

- **Hexagonal** because the same domain must serve a CLI, GUI, MCP server, and SDK without
  duplication, and because every external dependency (audio backend, LLM provider, storage,
  plugin formats) must be swappable (`00_Vision.md` §4).
- **CQRS** because reads (GUI meters, AI project inspection) and writes (commands mutating
  the project) have radically different shapes and consumers, and because an explicit
  command stream is what gives us undo/redo, determinism, audit, and AI safety for free.
- **Actor-style engine** because the real-time audio world cannot share locks with the
  async control world; they communicate exclusively by message passing (`10_Thread_Model.md`).
- **Modular monolith** (one process, many crates) rather than microservices: audio
  demands shared memory and microsecond budgets; the crate graph gives us the modularity
  without the network. Remote deployment remains possible later because all client access
  already goes through an RPC-capable service boundary.

### Alternatives considered

| Alternative | Rejected because |
|---|---|
| Layered monolith (classic DAW) | Couples domain to UI/tech; the exact failure mode we're building against |
| Microservices | Latency/serialization poison for audio; operational overhead irrelevant for local-first |
| Full event sourcing as the only state | Replay cost on large projects; we event-source *commands* but keep a materialized state (see §6) |
| Pure actor framework (e.g. actix everywhere) | Overkill for domain logic; actors reserved for the engine boundary where they earn their cost |

## 2. Layer Diagram and the Dependency Rule

```
┌────────────────────────────────────────────────────────────────────┐
│  CLIENTS            apps/cli · apps/desktop · apps/server(MCP+API) │
├────────────────────────────────────────────────────────────────────┤
│  SERVICE / APPLICATION LAYER                                       │
│    ProjectService · CompositionService · RenderService ·           │
│    WorkflowEngine · AgentRuntime · JobQueue                        │
│    (commands in → events out; owns transactions & orchestration)   │
├────────────────────────────────────────────────────────────────────┤
│  DOMAIN (pure, deterministic, almost dependency-free)              │
│    music-core · timeline · harmony · rhythm · midi ·               │
│    project-model · automation · mix-model                          │
│    + PORTS (traits): AudioOutput, Storage, LanguageModel,          │
│      PluginHost, Renderer, Clock, EventSink …                      │
├────────────────────────────────────────────────────────────────────┤
│  ADAPTERS (one crate per external tech)                            │
│    cpal-adapter · sqlite-adapter · fs-adapter · claude-adapter ·   │
│    onnx-adapter · clap-host-adapter · vst3-host-adapter ·          │
│    qdrant-adapter (future) · mcp-transport                         │
├────────────────────────────────────────────────────────────────────┤
│  ENGINE (separate real-time world; talks via lock-free queues)     │
│    audio-engine · audio-graph · dsp · instruments                  │
└────────────────────────────────────────────────────────────────────┘
```

**The Dependency Rule:** source dependencies point *inward only*.
Domain crates depend on `std` + a tiny approved set (`serde` for model types — see ADR-0004
tradeoff note). Adapters depend on domain (to implement its ports), never vice versa.
Clients depend on services, never on adapters directly (adapters are injected at
composition root — each `apps/*` main function is the only place concrete types are named).

Enforced mechanically: `cargo-deny` bans + a CI check that parses `cargo metadata` and
fails on any edge violating the layer table above (cheap to write, prevents drift forever).

## 3. Workspace Layout

```
music-os/
├── Cargo.toml                 # workspace, shared lints, shared deps via [workspace.dependencies]
├── apps/
│   ├── cli/                   # bin: `music`
│   ├── server/                # bin: MCP server + JSON-RPC/WS API (axum, tokio)
│   └── desktop/               # Tauri shell (React/TS in desktop/ui)
├── crates/
│   ├── core-types/            # ids, time types, errors — leaf crate, everything uses it
│   ├── music-core/            # symbolic model: notes, chords, scales, patterns
│   ├── harmony/  rhythm/      # theory engines (pure functions)
│   ├── midi/                  # SMF I/O + internal↔SMF mapping (wraps midly behind a port)
│   ├── timeline/              # bars/beats/ticks, tempo map, clips, markers
│   ├── project-model/         # aggregate: Project/Track/Clip + commands + events
│   ├── project-service/       # application layer: command handling, undo, snapshots
│   ├── composition/           # composer traits + rule-based composers
│   ├── arrangement/           # arranger traits + section engine
│   ├── audio-graph/           # node graph model, topo scheduling, latency compensation
│   ├── audio-engine/          # RT runtime: transport, voice mgmt, buffer pipeline
│   ├── dsp/                   # EQ, dynamics, delay, reverb… (no_std-friendly where possible)
│   ├── instruments/           # built-in synth + sampler
│   ├── plugin-api/            # stable traits for native plugins (see 09)
│   ├── plugin-host/           # discovery, lifecycle; clap-host/, vst3-host/ sub-adapters
│   ├── render/                # offline render pipeline + encoders behind ports
│   ├── ai-runtime/            # agent orchestration, sessions, budgets (see 06)
│   ├── ai-providers/          # claude/, openai/, gemini/, onnx/, llama/ adapter crates
│   ├── tools/                 # canonical tool registry: one definition → CLI cmd + MCP tool
│   ├── mcp-server/            # MCP protocol layer over tools/
│   ├── storage/               # Storage/Blob/Index ports + sqlite/, fs/ adapters
│   ├── config/                # layered config (global→workspace→project→runtime)
│   ├── events/                # domain event bus (in-proc broadcast)
│   └── telemetry/             # tracing setup, metrics
├── sdk/
│   ├── rust/                  # stable façade re-exports (semver promise lives here)
│   └── python/                # thin client over JSON-RPC/MCP
├── plugins/                   # first-party plugins built against plugin-api only
├── docs/  examples/  scripts/  tests/  benchmarks/  assets/  .github/
```

Rationale for granularity: a crate boundary = a reuse/replace boundary or a compile-time
firewall. `dsp` compiles without tokio; `music-core` compiles to wasm (future web);
`plugin-api` can be published independently with its own semver cadence.

## 4. The Tool Registry: one definition, every surface

The single most important reuse mechanism. Every capability is defined **once** in
`crates/tools` as:

```rust
pub trait Tool {
    fn spec(&self) -> ToolSpec;              // name, description, JSON Schema in/out
    async fn call(&self, ctx: &ServiceCtx, input: Value) -> Result<ToolOutput, ToolError>;
}
```

- `apps/cli` generates clap subcommands from `ToolSpec` (plus hand-tuned ergonomics).
- `mcp-server` publishes the same specs as MCP tools.
- `ai-runtime` hands the same specs to LLM providers as tool definitions.
- `sdk` exposes typed wrappers generated from the same schemas.

This is how "every CLI command corresponds to an MCP tool" (`01` FR-M1) is guaranteed
*structurally* instead of by convention. Tradeoff: JSON at the boundary costs some type
safety internally; mitigated by defining inputs as Rust structs with `schemars` +
`serde`, so the JSON Schema is derived, never hand-written.

## 5. Runtime Topology

One process hosts: tokio runtime (services, agents, RPC), the RT audio thread (only when
playback is active), and a worker pool (renders, analysis). See `10_Thread_Model.md`.
The desktop app runs the same services in-process via Tauri commands; `apps/server`
exposes them over stdio-MCP / WebSocket for external clients. State lives in the project
service regardless of which client connects — this is what makes S2 (`00` §5) work.

## 6. State & Data Flow (CQRS shape)

```
Client/Agent ──Command──▶ ProjectService
                            │ validate (domain invariants)
                            │ apply → new ProjectState (immutable snapshot, Arc'd)
                            │ append Command+Event to project log   ← undo/redo, audit, replay
                            ▼
                       EventBus ──▶ GUI refresh · AI context update · engine ReloadGraph msg
Reads: any client gets a cheap Arc<ProjectState> snapshot — no locks held during reads.
```

- **Materialized state + command log**, not full event sourcing: replay exists for
  determinism/undo, but opening a project loads the last snapshot then replays the tail.
- Engine receives *compiled* immutable graph/schedule structures, never domain objects
  (`04_Audio_Architecture.md` §5).

## 7. Error, Config, Telemetry Strategy (cross-cutting)

- **Errors:** `thiserror` per-crate error enums in domain/adapters; `ToolError` at the
  boundary carries a stable machine code (`E_PROJECT_NOT_FOUND`, `E_RT_OVERLOAD`, …) —
  the CLI maps codes to exit codes, MCP maps to structured tool errors (FR-CLI2).
  `anyhow` only in `apps/*`.
- **Config:** `crates/config` implements Global→Workspace→Project→Runtime layering; every
  service receives typed config structs, never reads files itself.
- **Telemetry:** `tracing` spans everywhere except the RT thread, which emits fixed-size
  event records through the lock-free queue for later formatting (NFR-1).

## 8. Future Scalability

- **Remote core:** because clients already speak service APIs, `apps/server` can move to
  another machine (render farm) without client changes — only transport config.
- **Collaboration:** the command log is the substrate for CRDT/OT experiments later.
- **Web client:** `music-core`/`timeline` kept wasm-compatible deliberately.
- **More providers/formats:** each is one new adapter crate; zero core edits (open/closed).
