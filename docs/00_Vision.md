# 00 — Vision

> **MusicOS is an operating system for autonomous music production — not a DAW.**

## 1. The Problem

Every mainstream DAW (FL Studio, Ableton Live, Logic, Reaper, Ardour) is built around one
assumption: **a human sits in front of a GUI and drives every action**. All capability —
composition, arrangement, mixing, rendering — is trapped inside a monolithic desktop
application. There is no stable, scriptable, machine-consumable surface.

Meanwhile, AI systems (LLM agents, symbolic music models, neural audio models) have become
capable of *planning* and *generating* music, but they have nowhere to *execute*: no
deterministic engine they can call as a set of tools, no project state they can safely
mutate, no render pipeline they can drive headlessly.

The gap is not a better DAW. The gap is **infrastructure**: the layer that sits between
intelligent planners (human or AI) and deterministic music execution.

## 2. What MusicOS Is

MusicOS is a set of **reusable Rust services** that expose every music-production
capability as an API:

```
        Human          AI Agent        External System
          │               │                  │
   ┌──────┴───────┬───────┴────────┬─────────┴──────┐
   │     CLI      │  Desktop (GUI) │  MCP / SDK / API│   ← clients (no logic)
   └──────┬───────┴───────┬────────┴─────────┬──────┘
          └───────────────┼──────────────────┘
                          ▼
                  MusicOS Core Services            ← all logic lives here
        (project · composition · arrangement ·
         MIDI · mixer · audio engine · render)
```

- The **CLI** is just a client.
- The **GUI** is just a client.
- The **MCP server** is just a client.
- **Every capability is a tool** — callable by a human, a script, or an LLM identically.

## 3. What MusicOS Is Not

| Not this | Because |
|---|---|
| A DAW clone | GUIs are replaceable clients; the core is the product. |
| An AI music toy | AI is a *planner* over deterministic services, not a black box that emits audio. |
| A SaaS product | Local-first, open-source. Cloud services are optional adapters. |
| A plugin (VST/CLAP) | MusicOS *hosts* plugins; it is the platform, not a guest. |

## 4. Core Beliefs (the "constitution")

These are the non-negotiable principles every design document downstream must honor.
Each is elaborated with rationale in the referenced doc.

1. **One core, many clients.** No logic in clients, ever. (→ `02_System_Architecture.md`)
2. **AI plans, services execute.** AI never mutates state directly; it issues commands
   through the same tool surface humans use. This keeps execution deterministic,
   auditable, and replayable. (→ `06_AI_Runtime.md`)
3. **Deterministic by default.** Same project + same commands → bit-identical symbolic
   state and (where feasible) identical rendered audio. Determinism is what makes an
   autonomous system debuggable and trustworthy. (→ `08_Project_Format.md`)
4. **Hexagonal everywhere.** The domain never names a concrete technology. CPAL, SQLite,
   Claude, Qdrant, VST3 — all live behind ports. (→ `02`, `09`)
5. **The audio thread is sacred.** No allocation, no locks, no syscalls, no surprises.
   (→ `04_Audio_Architecture.md`, `10_Thread_Model.md`)
6. **Plugin-first.** Instruments, effects, composers, exporters, and model providers are
   all plugins against stable trait interfaces. (→ `09_Plugin_System.md`)
7. **Open format.** Project files are inspectable, diffable, versioned, and documented.
   No lock-in — including from ourselves. (→ `08_Project_Format.md`)

## 5. North-Star Scenarios

These scenarios define "done" for the platform; every architectural choice is tested
against them.

**S1 — Autonomous production.**
`music compose "cinematic orchestral trailer, 90s, builds to a choir drop" --render out.wav`
An orchestrator agent plans sections, delegates to composer/arranger/mixer agents, each of
which calls deterministic tools; the result renders offline, headless, on CI-grade hardware.

**S2 — Conversational co-production.**
A user in Claude (or any MCP client) says "make the chorus bass punchier" — the MCP server
exposes `analyze_mix`, `set_eq`, `set_compressor` tools; the agent inspects, proposes,
applies, and the user hears the change in the desktop client *live*, because both clients
share one project service.

**S3 — Programmable pipeline.**
A label runs `music render --stems --format flac --loudness -14LUFS` across 500 projects
in CI. JSON output, machine-readable errors, reproducible results.

**S4 — Ecosystem growth.**
A third party ships a physical-modeling instrument as a CLAP plugin and a "jazz reharmonizer"
as a composer plugin — no fork of MusicOS required.

## 6. Why Now, Why Rust

- **Why now:** MCP standardized the tool surface between LLMs and applications (2024–25);
  agentic frameworks (ComposerX, WeaveMuse — see `13_Research.md`) proved multi-agent music
  planning works but lack an execution substrate; the Rust audio ecosystem (CPAL, NIH-plug,
  CLAP) matured enough to host real-time engines.
- **Why Rust:** the only mainstream language that gives us (a) C++-class real-time audio
  performance, (b) memory safety across a plugin/FFI boundary-heavy codebase, (c) fearless
  concurrency for the engine/agent split, and (d) a first-class workspace story for a
  20+ crate modular monorepo. Alternatives considered: C++ (unsafe, slower iteration,
  weaker tooling), Go (GC pauses unacceptable on the audio thread), Zig (ecosystem too
  young for plugin hosting and MCP).

## 7. Success Criteria (3-year horizon)

| Dimension | Target |
|---|---|
| Reuse | CLI, GUI, MCP, SDK ship with **zero** duplicated domain logic |
| Determinism | Symbolic replay is bit-identical; offline renders reproducible per platform |
| Performance | Real-time engine stable at 128-sample buffers, 48 kHz (see `11_Performance_Goals.md`) |
| Extensibility | Third-party instrument, effect, composer, and model-provider plugins exist |
| Community | External contributors can add a crate/plugin without touching core |

## 8. Document Map

| Doc | Question it answers |
|---|---|
| `01_Product_Requirements.md` | What must it do, for whom? |
| `02_System_Architecture.md` | How is the system decomposed? |
| `03_Domain_Model.md` | What are the concepts and their invariants? |
| `04_Audio_Architecture.md` | How does real-time and offline audio work? |
| `05_Music_Core.md` | How is music represented and manipulated symbolically? |
| `06_AI_Runtime.md` | How do agents plan and act? |
| `07_MCP_Architecture.md` | How is capability exposed to the AI ecosystem? |
| `08_Project_Format.md` | How is state persisted, versioned, shared? |
| `09_Plugin_System.md` | How is the system extended? |
| `10_Thread_Model.md` | Who runs where, and how do they talk? |
| `11_Performance_Goals.md` | What are the numbers we hold ourselves to? |
| `12_Development_Roadmap.md` | In what order do we build it? |
| `13_Research.md` | What did we learn from prior art, and what did we adopt/reject? |
| `adr/` | Running log of significant decisions |
