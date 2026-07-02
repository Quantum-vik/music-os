# 09 — Plugin System

Status: Draft v1 · Depends on: `02`, `04` · Key ADRs: ADR-0013 (CLAP first), ADR-0014 (native plugins = compiled-in first, ABI later)

## 1. Two Meanings of "Plugin", One Philosophy

MusicOS is extended along **every** axis, not just audio:

| Extension point | Trait (in `plugin-api`) | Examples |
|---|---|---|
| Instruments | `Instrument: Processor` | synths, samplers, physical models |
| Effects | `Effect: Processor` | EQs, reverbs, weird DSP |
| Composers | `Composer` (05 §4) | jazz reharmonizer, neural melody model |
| Arrangers | `Arranger` | genre form templates |
| Analyzers | `Analyzer` | key detection, mix critique metrics |
| Model providers | `LanguageModel`/`Embedder` (06 §2) | new LLM/local model backends |
| Exporters | `Exporter` | encoders, DAW-bridge writers |
| Storage/vector | `Storage`/`VectorStore` | Qdrant, S3 |

Everything above is *host-side extensibility of MusicOS itself*. Separately, the **audio
plugin host** loads third-party CLAP/VST3 plugins into the engine graph. Both funnel into
the same registries so discovery/config/UX are uniform (`music plugins list` shows all).

## 2. Delivery Mechanisms for Native Plugins (ADR-0014)

Phased, because a stable Rust ABI does not exist:

1. **v1 — compiled-in (static) plugins.** First-party and in-tree community plugins are
   crates in `plugins/` implementing `plugin-api` traits, registered at the composition
   root (feature-gated). Zero ABI risk, full type safety, trivially testable. The
   "registry + trait" shape is identical to later phases, so plugin *authors'* code doesn't
   change — only packaging does.
2. **v1.x — out-of-process extensions.** For AI/tooling plugins where microseconds don't
   matter: sidecar processes speaking a versioned JSON-RPC (or… MCP itself — a composer
   plugin can literally be an MCP server; the tool-registry duality makes this nearly free).
3. **v2 — dynamic native loading**, if demand proves it: either a C-ABI shim of
   `plugin-api` (stable but verbose) or `abi_stable`/`stabby`-style crates, or — most
   likely — **CLAP's extension mechanism as our ABI** for audio-adjacent plugins, since we
   already host CLAP (below) and CLAP is designed exactly for stable C-ABI extensibility.

Rejected for v1: dylib loading of `rustc`-ABI plugins (breaks on every compiler bump);
wasm plugins for DSP (tempting for sandboxing; deferred — wasm SIMD/perf and param-UX
questions; excellent candidate for *composer* plugins later).

## 3. Audio Plugin Hosting

### CLAP first (ADR-0013)
- MIT-licensed, C ABI designed for hosting, sample-accurate events, thread-pool
  cooperation, per-note modulation — technically the best fit for our engine model, and
  license-clean for an open-source host. Adapter: `clap-host` crate over `clack`(evaluate)/`clap-sys`.
- The CLAP adapter defines our internal `HostedPlugin` shape: activate/process lifecycle
  mapping onto our prepare/process contract (`04` §6), parameter table bridged to atomic
  param cells, event I/O converted at the boundary.

### VST3 second
- Ecosystem reach demands it eventually; GPLv3-or-agreement licensing and a hairier API
  argue for *after* CLAP proves the host abstraction. Lives in `vst3-host` crate so its
  license never touches core (`01` §5). AU/LV2: platform-scoped, post-v1, same port.

### Discovery, scanning, metadata
- Standard OS paths + config; **scan in a subprocess** (industry-standard: plugin scans
  crash) with a quarantine list for misbehaving binaries; results cached in SQLite
  (`storage`), keyed by file hash + mtime.

### Sandboxing (post-v1, design reserved)
Out-of-process hosting: plugin server process per (plugin|group), shared-memory audio
rings, one block extra latency compensated by PDC (`04` §9). Crash = voice drops out,
engine survives (NFR-7). v1 hosts in-process with `catch_unwind` at FFI edges — honest
tradeoff: stability best-effort until sandbox lands (roadmap phase 5).

## 4. Parameter & Preset Model

- Unified `ParamAddr` + `ParamInfo { range, unit, flags, stepping }` across built-ins,
  CLAP, VST3 — automation (`03`) and MCP tools (`set_param`) address every parameter the
  same way. Normalized↔plain conversions live in host adapters (formats disagree; the
  domain sees plain values + declared ranges).
- Presets: our own JSON preset envelope wrapping format-native chunks (opaque blobs for
  hosted plugins, structured state for native ones), content-addressed in the asset store.

## 5. GUI Hosting

Plugin GUIs are native windows (CLAP/VST3 embed via raw-window-handle) parented into the
Tauri shell where the platform allows; headless contexts (CLI/MCP) always work without UI
— every parameter is settable via tools *by construction* (AI can't click knobs).

## 6. Safety & Trust Model

- Hosted binary plugins are unsandboxed native code in v1 — documented loudly; quarantine +
  subprocess scanning limit blast radius; sandbox is the roadmap answer.
- Native (in-tree) plugins pass the same CI gates as core (clippy/tests/audit).
- Out-of-process extensions (mechanism 2) get OS-level user permissions only; no implicit
  filesystem access through MusicOS APIs beyond project scope.

## 7. Testing

- `plugin-api` ships a **conformance harness**: golden lifecycle sequences (activate →
  process silence/impulse/notes → param sweeps → deactivate) any plugin must pass; doubles
  as the reference documentation and keeps the built-ins honest.
- Host adapters tested against known-good open plugins (Surge XT, Vital's CLAP, Airwindows)
  in CI where licenses/binaries permit; otherwise local test suites documented in
  `scripts/`.

## 8. Future Evolution

- Wasm composer/analyzer plugins (sandboxed, portable, registry-distributed).
- A `music plugins install` registry (crates.io-style) once packaging (mechanism 3) settles.
- NIH-plug *export* of our built-in DSP as plugins for other DAWs — the `dsp` crate's
  engine-independence (`02` §3) exists partly for this.
