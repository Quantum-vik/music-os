# 12 — Development Roadmap

Status: Draft v1 · Multi-month plan. Each phase has **exit criteria** — we do not advance on vibes.
Ordering principle: *walk the skeleton end-to-end early* (CLI → domain → render pipeline first), because integration risk, not feature count, kills platform projects.

## Phase 0 — Foundations (repo before features)

Workspace scaffold per `02` §3 (empty crates with docs + `#![deny(missing_docs)]`);
`Justfile` (`setup/build/test/lint/fmt/bench/docs/run/clean`); bootstrap scripts (rustup,
clippy, rustfmt, nextest, cargo-audit, cargo-deny, pre-commit hooks, node, python venv);
GitHub Actions matrix (fmt, clippy -D warnings, nextest, audit/deny, docs build) on
Linux/macOS/Windows; layer-rule CI check (`02` §2); ADR process live; LICENSE (Apache-2.0
OR MIT), CONTRIBUTING, CoC.
**Exit:** `just setup && just test` green on all 3 platforms in CI; a trivial cross-crate
call (`cli → project-service → core-types`) ships to prove the layering.

## Phase 1 — Symbolic Core (the deterministic heart)

`core-types` (Tick/Pitch/etc per `03` §2) → `music-core` (Note/Pattern) → `harmony`/
`rhythm` (theory engine, `05` §2) → `midi` (SMF round-trip) → `timeline` (tempo map,
tick↔sample math) → `project-model` (aggregate + commands/events + invariants) →
`project-service` (ProjectActor, undo, snapshots) → `storage` ports + fs/sqlite adapters →
project format v0 (`08`) → `config` layering → tool registry v0 (`02` §4) → CLI v0
(`music init/import/export/transpose/quantize/log/undo`, `--json` from day one).
Testing per `05` §7 (property/snapshot/fuzz).
**Exit:** create → edit via commands → save → reopen → replay-identical (hash-equal);
SMF round-trip property suite green; CLI usable for real MIDI wrangling.

## Phase 2 — Offline Audio (hear something, deterministically)

`audio-graph` (compile: topo sort, buffer liveness, PDC) → `dsp` v0 (gain, pan, biquad EQ,
compressor, limiter, delay; prepare/process contract + no-alloc guard) → `instruments` v0
(subtractive synth, basic sampler) → `render` (offline driver, WAV/FLAC, EBU R128
normalize, stems) → mixer model wired (tracks/buses/sends) → `music render` + `analyze`.
Benchmarks stood up (`11` §4) incl. determinism gate.
**Exit:** reference project renders correct, deterministic (hash-stable), ≥10×RT on REF-A;
S3 pipeline scenario (batch render in CI) demonstrated.

## Phase 3 — AI Runtime + MCP (the differentiator)

`ai-runtime` ports (`06` §2) → MockModel/ReplayModel + offline test harness → **Claude
Agent SDK adapter** (auth, streaming, tools, sessions, retries) → rule-based composers
behind `Composer` (already usable without AI) → orchestrator + composer/reviewer loop with
budgets → `AiRunRecord` logging → `mcp-server` over the registry (stdio; tools+resources+
progress) → CLI `music compose/arrange` (works with `--no-ai` rule-based, or full agentic).
**Exit:** **S1 end-to-end** — brief → multi-agent plan → deterministic tools → rendered
WAV, headless; MCP conformance suite green; S2 partially (MCP tool session against a live
project); replay without model calls reproduces the project exactly.

## Phase 4 — Real-Time Engine (playback)

CPAL adapter + RT thread (`04` §2 rings, graph swap, param atomics) → transport/event
scheduler (sample-accurate splits) → voice manager → xrun/overload policy → RT test rig
(allocator guard, soak, glitch-on-swap tests) → `music play` (CLI transport!) → RT
budgets from `11` §2 tracked.
**Exit:** reference project plays at 128/48k on REF-A within budgets; edit-during-playback
(graph swap) glitch-free; doctor reports health.

## Phase 5 — Plugin Hosting + Desktop

CLAP host adapter (+ subprocess scanner, quarantine) → param/preset unification (`09` §4)
→ VST3 adapter (license-isolated) → Tauri desktop shell v0: timeline, mixer, transport
bound to the same services (in-proc), event-bus-driven meters → plugin GUI embedding →
(stretch) out-of-process sandbox prototype.
**Exit:** third-party CLAP instrument plays in a project from both GUI and CLI; S2 fully:
change made via Claude/MCP is heard live in the desktop app.

## Phase 6 — Ecosystem & Hardening (v1.0)

Python SDK; `plugin-api` conformance harness published; format v1 freeze + migration
corpus; docs site (mdBook from `/docs` + cargo doc); fuzzing/soak/chaos in nightly CI;
release engineering (signed binaries, `cargo-dist`, checksums, changelogs); triage/security
policy; ONNX composer-plugin proof of concept; loudness-targeted mastering chain.
**Exit:** v1.0 acceptance list in `01` §6 fully green; an external contributor has landed
a plugin without core changes (rehearsed, not hoped).

## Post-v1 Backlog (explicitly deferred, design seams already reserved)

Parallel RT graph (`04` §3) · audio input/recording · time-stretch/pitch-shift · sandboxed
plugin processes (`09` §3) · local LLM adapters (llama.cpp) · vector memory (Qdrant) ·
collaboration/CRDT experiments over the command log (`08` §4) · DAW bridges (FL/Ableton
export) · wasm plugins · web client over `music-core` wasm · MPE/expression lanes ·
video/SMPTE sync.

## Working Agreements (apply to every phase)

- The 11-step feature workflow (research → design → review → traits → ports → adapters →
  tests → implement → benchmark → document → refactor) is the PR template.
- Significant decisions get an ADR **before** implementation; architecture review for
  anything crossing a layer boundary.
- No phase ships with red quality gates (fmt/clippy/tests/audit/deny/docs — NFR-8/9).
- Each phase ends with a written retro amending the *next* phase's plan — this roadmap is
  a living document, versioned like code.

## Top Risks & Mitigations

| Risk | Mitigation |
|---|---|
| RT engine hardest last-mile (Phase 4 slip) | Offline-first ordering de-risks DSP/graph correctness before RT constraints; RT test rig built *with*, not after, the engine |
| Claude SDK/runtime instability or auth changes | Adapter isolation + raw-API fallback + MockModel CI (never blocked on live AI) |
| MCP spec churn | rmcp upstream + thin skin over registry (`07` §2) |
| Plugin hosting rabbit hole | CLAP-only first; VST3 strictly after host abstraction proven |
| Scope gravity toward "DAW features" | Personas P1/P3/P4 first (`01` §1); GUI is Phase 5 by design |
| Solo/small-team burnout | Phases sized to produce a usable artifact each (CLI MIDI tool → renderer → AI producer → player → host) — momentum via shipping |
