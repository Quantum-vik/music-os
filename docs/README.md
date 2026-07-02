# MusicOS — Design Documentation

MusicOS is an **operating system for autonomous music production** — reusable Rust
services where the CLI, desktop app, MCP server, SDKs, and AI agents are all equal
clients of one core. Start with [`00_Vision.md`](00_Vision.md).

## Reading Order

| Doc | Contents |
|---|---|
| [00_Vision](00_Vision.md) | What MusicOS is/isn't, core beliefs, north-star scenarios |
| [01_Product_Requirements](01_Product_Requirements.md) | Personas, FR/NFR, non-goals, v1 acceptance |
| [02_System_Architecture](02_System_Architecture.md) | Hexagonal layers, workspace layout, tool registry, CQRS flow |
| [03_Domain_Model](03_Domain_Model.md) | Bounded contexts, value objects, aggregates, commands/events |
| [04_Audio_Architecture](04_Audio_Architecture.md) | RT engine, graph compile+swap, DSP, offline render |
| [05_Music_Core](05_Music_Core.md) | Symbolic model, theory engine, transforms, generators, interchange |
| [06_AI_Runtime](06_AI_Runtime.md) | Provider ports, Claude Agent SDK, agent topology, determinism boundary |
| [07_MCP_Architecture](07_MCP_Architecture.md) | Tools/resources/prompts, transports, long-running jobs |
| [08_Project_Format](08_Project_Format.md) | Bundle format, command log, versioning/migration, assets |
| [09_Plugin_System](09_Plugin_System.md) | Extension points, native plugin phases, CLAP/VST3 hosting |
| [10_Thread_Model](10_Thread_Model.md) | Three worlds, channel inventory, RT contract, shutdown |
| [11_Performance_Goals](11_Performance_Goals.md) | Budgets, methodology, perf-correctness gates |
| [12_Development_Roadmap](12_Development_Roadmap.md) | Phases 0–6 with exit criteria, risks |
| [13_Research](13_Research.md) | Prior-art findings → decisions derived |
| [adr/](adr/README.md) | Decision log (ADR-0001…0015) + template |

## Ground Rules

1. Design docs are living; ADRs are immutable (supersede, don't rewrite).
2. No significant implementation without a design section and, if hard to reverse, an ADR.
3. Phase 1 mandate: **documentation precedes code** — implementation begins per
   [12_Development_Roadmap](12_Development_Roadmap.md) Phase 0 only after these docs are reviewed.
