# 01 — Product Requirements

Status: Draft v1 · Depends on: `00_Vision.md`

## 1. Personas

| ID | Persona | Primary interface | Needs |
|---|---|---|---|
| P1 | **AI agent** (Claude via MCP, custom agents) | MCP tools | Complete, well-described, deterministic tool surface; structured errors; streaming progress |
| P2 | **Producer / musician** | Desktop app | Low-latency playback, plugin hosting, familiar timeline/mixer mental model |
| P3 | **Developer / hacker** | CLI + SDK | Scriptable everything, JSON output, stable APIs, good docs |
| P4 | **Pipeline operator** (label, game studio, CI) | CLI headless | Batch rendering, reproducibility, machine-readable errors, no GUI dependency |
| P5 | **Plugin/extension author** | Plugin SDK | Stable trait ABI, docs, examples, test harness |
| P6 | **Researcher** | Python SDK / MCP | Symbolic import/export, dataset-friendly formats, evaluation hooks |

Priority order for v1: **P1 > P3 > P4 > P2 > P5 > P6.** Rationale: the differentiator is
the AI-native tool surface (P1); the CLI (P3/P4) is the cheapest client that exercises the
whole core; the desktop app (P2) is the most expensive client and rides on a proven core.

## 2. Functional Requirements

Grouped by subsystem. **MUST** = v1 blocker, **SHOULD** = v1 target, **MAY** = post-v1.

### FR-P: Project management
- FR-P1 (MUST) Create/open/save projects in the open bundle format (`08_Project_Format.md`).
- FR-P2 (MUST) Tracks (MIDI + audio), clips, regions, markers, loop points, tempo map, time signatures.
- FR-P3 (MUST) Unlimited undo/redo via command log; snapshots.
- FR-P4 (SHOULD) Asset management (samples, presets) with content-addressed storage.
- FR-P5 (MAY) Project diff/merge tooling.

### FR-C: Composition (symbolic)
- FR-C1 (MUST) Generate/edit chord progressions, melodies, basslines, drum patterns as MIDI.
- FR-C2 (MUST) Music-theory operations: scales, keys, transposition, quantize, humanize, voice-leading checks.
- FR-C3 (MUST) MIDI import/export (SMF 0/1); (SHOULD) MusicXML import/export.
- FR-C4 (SHOULD) Arrangement operations: section templates (intro/verse/chorus/bridge/outro), clip duplication with variation.
- FR-C5 (MAY) Neural symbolic generation via ONNX models as a composer plugin.

### FR-A: Audio engine
- FR-A1 (MUST) Offline (faster-than-real-time) rendering of a project to WAV/FLAC; (SHOULD) MP3/OGG; stems.
- FR-A2 (MUST) Real-time playback through system audio with sample-accurate transport.
- FR-A3 (MUST) Graph-based mixer: tracks, buses, sends, inserts, master; automation of any parameter.
- FR-A4 (MUST) Built-in DSP suite: gain, pan, EQ, compressor, limiter, delay, reverb, saturation.
- FR-A5 (MUST) Built-in instruments: polyphonic subtractive synth, sampler.
- FR-A6 (SHOULD) CLAP plugin hosting; (MAY) VST3 hosting; (MAY) out-of-process sandboxing.
- FR-A7 (SHOULD) Loudness measurement (EBU R128) and loudness-targeted mastering.
- FR-A8 (MAY) Time-stretch / pitch-shift of audio clips.

### FR-AI: AI runtime
- FR-AI1 (MUST) Claude Agent SDK integration: subscription auth, streaming, tool calling, session persistence, retry/error recovery.
- FR-AI2 (MUST) Provider abstraction so OpenAI/Gemini/local (ONNX, llama.cpp) providers are adapters (`06_AI_Runtime.md`).
- FR-AI3 (MUST) Orchestrator + specialist agents (composer, arranger, mixer, reviewer, mastering) with bounded budgets.
- FR-AI4 (MUST) AI acts only through the command/tool surface — never direct state mutation.
- FR-AI5 (SHOULD) Conversation/session persistence per project; (MAY) semantic memory via vector store.

### FR-M: MCP server
- FR-M1 (MUST) Every CLI capability exposed as an MCP tool with JSON-schema'd inputs/outputs.
- FR-M2 (MUST) stdio transport; (SHOULD) HTTP/WebSocket transport with auth.
- FR-M3 (SHOULD) MCP resources for project state (read) and progress notifications for long jobs.

### FR-CLI: Command line
- FR-CLI1 (MUST) `music init|compose|arrange|render|mix|analyze|plugins|doctor` (see `12_Development_Roadmap.md` for phasing).
- FR-CLI2 (MUST) `--json` machine output on every command; typed error codes; non-zero exit on failure.
- FR-CLI3 (MUST) Progress bars (TTY) / progress events (JSON mode); `--verbose`/`-v` tracing.
- FR-CLI4 (SHOULD) Batch mode (glob/stdin project lists); shell completion.

### FR-D: Desktop
- FR-D1 (SHOULD) Tauri + React shell: timeline, mixer, transport, plugin UI hosting.
- FR-D2 (SHOULD) Desktop talks to the same service layer (in-process or local RPC) — no private APIs.

### FR-S: SDK & config
- FR-S1 (MUST) Rust SDK crate (`sdk/`) re-exporting stable service APIs.
- FR-S2 (SHOULD) Python SDK over the JSON-RPC/MCP surface.
- FR-S3 (MUST) Layered config: global → workspace → project → runtime (env/flags); TOML primary, YAML/JSON accepted.

## 3. Non-Functional Requirements

| ID | Requirement | Target (see `11_Performance_Goals.md` for full budgets) |
|---|---|---|
| NFR-1 | Real-time safety | Zero allocation/locks/blocking on audio thread — enforced by design + debug assertions |
| NFR-2 | Latency | Stable at 128 samples @ 48 kHz on reference hardware; ≤ 1 xrun/hour |
| NFR-3 | Offline render speed | ≥ 10× real-time for a 16-track reference project |
| NFR-4 | Determinism | Symbolic ops bit-identical across runs & platforms; audio renders bit-identical per platform+version |
| NFR-5 | Startup | CLI cold start ≤ 150 ms (no engine); engine init ≤ 2 s |
| NFR-6 | Portability | Linux, macOS, Windows tier-1; CI builds all three |
| NFR-7 | Reliability | A crashing plugin (sandboxed mode) must not kill the engine; AI failures degrade gracefully |
| NFR-8 | Quality gates | rustfmt, clippy (deny warnings), nextest, cargo-audit, cargo-deny green on every PR |
| NFR-9 | Docs | Every public API documented; `#![deny(missing_docs)]` in core crates |
| NFR-10 | Security/privacy | Local-first; nothing leaves the machine except explicit AI provider calls; keys via OS keychain/env |

## 4. Explicit Non-Goals (v1)

- Live performance features (clip launching, live looping) — format leaves room, engine does not target it.
- Video scoring/sync (SMPTE) — post-v1.
- Collaborative real-time editing (CRDTs) — format is event-sourced partly to enable this *later*; not v1.
- Mobile clients.
- Hosting our plugins *inside other DAWs* (we are a host, not a guest) — revisit post-v1; the DSP crates are structured so NIH-plug export is possible.
- Audio recording from inputs (playback/render first; recording is post-v1).

## 5. Constraints & Assumptions

- Rust stable only; no nightly features in shipped crates.
- Claude Code SDK is the reference AI integration; a subscription-authenticated local
  `claude` runtime is assumed available (adapter degrades to API-key auth).
- Neural inference is optional at runtime: everything must work with zero models installed
  (rule-based composers as fallback) — this keeps CI and P4 deterministic.
- VST3 licensing (GPLv3 or Steinberg agreement) — MusicOS core stays licensed to permit
  both; VST3 host adapter isolated in its own crate so its licensing never contaminates core. CLAP (MIT) is the first-class citizen.

## 6. Acceptance: definition of "v1 done"

1. Scenario S1 (`00_Vision.md`) runs end-to-end headless on all three platforms.
2. Scenario S3 runs in GitHub Actions using the release CLI binary.
3. MCP conformance: tools usable from Claude Code against a real project.
4. All MUST requirements green; performance budgets in `11_Performance_Goals.md` met on reference hardware.
