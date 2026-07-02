# 13 — Research

Status: Draft v1 · Companion: `../.. /Agentic_Music_OS_Research_Roadmap.md` (reading list this synthesizes).
Format per area: findings → **design decisions derived** (with doc cross-refs).

## 1. DAW & Real-Time Audio Architectures

**Ardour** (C++, libre): session/engine split; graph-based routing with a "process graph"
executed by pinned RT workers; rt-safe operations via lock-free messaging to the butler
(disk) thread; plugin scans crash → they subprocess them. Weakness we avoid: deeply
GUI-entangled session logic accumulated over decades.
**Reaper** (closed, observed behavior/forums): anticipative FX processing (render-ahead on
worker threads), f64 pipeline, extreme robustness to plugin misbehavior, tiny binary —
proof that disciplined engineering beats framework weight.
**JUCE / Tracktion Engine**: `AudioProcessorGraph` rebuild-and-swap; Tracktion proves an
*engine as a library* is viable (engine/UI split akin to ours) and that unifying offline +
realtime rendering paths matters; its `tracktion_graph` rewrite exists precisely because
retrofitting parallelism into a graph executor is painful — they encode dependency levels
in the compiled schedule (we copy that seam, `04` §3).
**NIH-plug / RustAudio (CPAL, rodio, fundsp, dasp)**: NIH-plug demonstrates idiomatic
Rust plugin DSP with sample-accurate event splitting (we adopt, `04` §5); CPAL is the only
credible cross-platform Rust audio I/O today (ADR-0006); rodio is prototyping-only (its
mixer model is too simple to build on); fundsp elegant but its graph-as-types model fights
dynamic project graphs — we use it, if at all, inside individual nodes.
**Ross Bencina's "real-time audio 101" canon**: the no-alloc/no-locks/no-syscalls
commandments — restated as our enforceable RT contract (`10` §4).

→ **Decisions:** compile+atomic-swap immutable graphs (ADR-0005); engine-as-library with
clients on top; unified offline/RT executor; subprocess plugin scanning; CPAL adapter
behind an `AudioOutput` port; single-threaded RT v1 with dependency-leveled schedule for
future parallelism.

## 2. Agentic Music Systems (the closest prior art)

**ComposerX (2024)**: group chat of role agents (leader, melody, harmony, instrument,
reviewer) writing ABC notation; multi-agent + critique dramatically improves musicality
over single GPT-4 calls; but free-chat coordination is token-hungry and unreproducible.
**ByteComposer (2024)**: human-composer workflow as *fixed expert stages* (concept →
draft → self-evaluation → refine) — a pipeline, not a chat; more controllable, converges.
**CoComposer (2025)**: five collaborating agents; better editability/duration than
single-agent; still text-protocol-fragile between agents.
**WeaveMuse (2025)**: closest to an "OS": an orchestrator invoking specialized *tools/
models* (understanding, symbolic generation, audio synthesis) under resource-aware
scheduling — validates tool-orchestration over end-to-end generation, and open modular
deployment.
→ **Decisions (`06` §4):** role-specialized agents **with typed plan-DAG orchestration
instead of free chat** (ByteComposer's structure + ComposerX's critique loop, WeaveMuse's
tool-centric execution); reviewer agent with *symbolic analysis tools* rather than
LLM-only judgment; hard budgets; deterministic tool substrate as the agents' instrument —
the gap all four papers leave open is exactly the execution layer MusicOS builds.

## 3. Music LLMs & Symbolic Generation

**ChatMusician**: LLaMA2 continually pretrained on **ABC notation** — text-compatible
symbolic music beats specialized token vocabularies for LLM affinity; motivates our terse
JSON/ABC-friendly pattern encodings (`05` §5) and optional ABC I/O.
**MuseCoCo / text-attribute control**: controllability comes from *structured attributes*,
not prose — our `Brief` type (`05` §4) is exactly that lesson.
**REMI / Music Transformer / Compound Word / Museformer**: token design determines what
models learn (position/duration beats note-on/off; bar-aware structure helps long-range
form) — informs any future neural composer plugin's tokenizer and our self-similarity
form analysis (`05` §6).
**MusicGen / MusicLM / Stable Audio (audio-domain)**: powerful but uneditable end-to-end;
for a *production* system they are, at best, future texture/sample generator plugins —
symbolic-first is the controllable path (core premise of `00`).

## 4. Agent Frameworks & MCP

**ReAct/Toolformer** → interleaved reasoning + tool use is the base loop (Agent SDK gives
it to us). **HuggingGPT** → plan-then-execute over typed tool specs scales better than
open chat (adopted, `06` §4). **AutoGen/CAMEL/MetaGPT** → role prompts and structured
handoffs work; unbounded conversations don't ship products (budgets, typed plans).
**Voyager** → skill libraries hint at our future memory of successful briefs/critiques
(`06` §5). **MCP** → the standardization event making "every capability is a tool" viable
across hosts; resources/prompts/progress used per `07`.

## 5. Rust Engineering Practice (for this codebase's shape)

Workspace-of-many-crates with `[workspace.dependencies]` and shared lint tables (rust-analyzer,
Bevy, Tokio precedents) → `02` §3. Bevy's plugin/registry ergonomics inform our
compiled-in plugin registration (ADR-0014 phase 1). Tokio's discipline: bounded channels,
no runtime creation in libraries, `spawn_blocking` for sync I/O → `10` §6. `thiserror`
domain / `anyhow` binary split; `#![deny(missing_docs)]` in public crates; proptest +
cargo-fuzz on parser surfaces; criterion with tracked baselines → `11`.
Lock-free: SPSC rings (`rtrb`) + atomic swap of `Arc<immutable>` cover 100% of our RT
communication needs — full lock-free structures (queues/maps) deliberately avoided as
complexity without payoff.

## 6. Standards & Formats

**SMF/MIDI 1.0**: integer PPQ + tempo-map derivation of wall time (adopted wholesale,
ADR-0004); MIDI 2.0/MPE watched, modeled as optional expression lanes later (`03` §9).
**MusicXML**: interchange-only subset (full spec is unimplementable-in-anger; MuseScore
compatibility as the practical bar) — `05` §5. **CLAP vs VST3**: CLAP's MIT license, C ABI,
sample-accurate events and threading contract make it the correct first host target
(ADR-0013); VST3 follows for reach. **EBU R128/LUFS**: the loudness contract for mastering
and batch pipelines (`04` §7).

## 7. Open Questions (tracked; each gets an ADR when resolved)

1. Persistent-collection strategy for `ProjectState` snapshots (`im` vs Arc-slotmap
   clone-on-write) — benchmark in Phase 1.
2. CLAP host library: `clack` maturity vs raw `clap-sys` — spike in Phase 5.
3. MusicXML subset boundary — define against a corpus of MuseScore/Dorico exports.
4. Desktop meter/waveform transport: Tauri IPC vs shared-memory channel — measure in Phase 5.
5. Whether reviewer-agent scoring needs a learned model (vs rule metrics) — collect run
   telemetry from Phase 3 before deciding.
