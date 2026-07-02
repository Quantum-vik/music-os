# Architecture Decision Records

One ADR per significant, hard-to-reverse decision. Format: **Context → Decision →
Consequences (incl. rejected alternatives)**. Status: Proposed → Accepted → Superseded(-by).
New ADRs: copy `template.md`, number sequentially, link from the index below and from the
affected design doc. ADRs are immutable once Accepted — supersede, don't edit history.

| # | Title | Status | Detailed in |
|---|---|---|---|
| 0001 | Hexagonal architecture; dependency rule enforced in CI | Accepted | `02` §1–2 |
| 0002 | Single Cargo workspace, modular monolith, crate-per-boundary | Accepted | `02` §3 |
| 0003 | CQRS-shaped application layer: commands + event log + materialized state (not full event sourcing) | Accepted | `02` §6, `03` §4 |
| 0004 | Musical time = integer ticks @ 960 PPQ; wall time derived via tempo map; no floats in musical time | Accepted | `03` §2 |
| 0005 | Audio graph compiled off-thread to immutable schedule; atomic Arc swap; deallocation off-RT | Accepted | `04` §2–3 |
| 0006 | CPAL as v1 audio I/O adapter behind `AudioOutput` port | Accepted | `04` §8 |
| 0007 | f32 internal audio pipeline with f64 accumulators where analysis demands | Accepted | `04` §6 |
| 0008 | Build our own music-theory crates (`harmony`, `rhythm`) rather than wrapping existing ones | Accepted | `05` §2 |
| 0009 | Claude Agent SDK as primary AI integration; raw-API adapters as fallback | Accepted | `06` §3 |
| 0010 | AI acts exclusively through validated commands/tools; replay reproduces projects without model calls | Accepted | `06` §1, §6 |
| 0011 | MCP via official `rmcp` SDK; tools/resources derived from the single tool registry | Accepted | `07` §2 |
| 0012 | Project = bundle directory; JSON canonical state + JSONL command log; SQLite as rebuildable cache only | Accepted | `08` §2 |
| 0013 | CLAP is the first-class hosted plugin format; VST3 second, license-isolated | Accepted | `09` §3 |
| 0014 | Native MusicOS plugins: compiled-in (v1) → out-of-process (v1.x) → stable-ABI dynamic (v2, likely via CLAP extensions) | Accepted | `09` §2 |
| 0015 | Three-world thread model (async/worker/RT); message passing only across worlds; bounded channels with named overflow policies | Accepted | `10` |

Amendment to ADR-0011 (2026-07-03): the v0 MCP server is a minimal
hand-rolled stdio JSON-RPC implementation (tools surface only, zero new
dependencies) rather than `rmcp` — adopted to ship the tools surface without
taking on SDK churn. `rmcp` replaces it when resources, HTTP transport, or
auth land; the tool-registry abstraction keeps that swap inside
`crates/mcp-server`.

Pending (spikes scheduled, see `13` §7): persistent-collection strategy for snapshots;
CLAP host library choice; MusicXML subset boundary; desktop meter transport.
