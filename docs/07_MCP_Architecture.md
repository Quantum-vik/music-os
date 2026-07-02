# 07 — MCP Architecture

Status: Draft v1 · Depends on: `02` (tool registry), `06` · Key ADRs: ADR-0011 (rmcp + registry-derived tools)

## 1. Role of MCP in MusicOS

MCP is how the *outside* AI ecosystem (Claude, IDEs, other agent hosts) drives MusicOS.
It is deliberately a **thin protocol skin over the tool registry** — the server contains
no logic beyond protocol mapping. Internal agents (`06`) call tools in-process; external
agents call the identical tools over MCP. One capability surface, two entry points.

```
Claude / IDE / any MCP host
        │  stdio  or  Streamable HTTP(+WS)
        ▼
apps/server (mcp-server crate, rmcp)
        │  ToolSpec ↔ MCP tool listing · JSON in/out · progress notifications
        ▼
crates/tools  ──▶  services  ──▶  domain
```

## 2. SDK Choice (ADR-0011)

**`rmcp` (the official Rust MCP SDK)** for protocol handling; tools/resources/prompts are
*derived from our registry*, not hand-registered with SDK macros. Rationale: protocol
churn (MCP spec is evolving) is absorbed by maintained upstream code; deriving from the
registry preserves the single-definition guarantee (FR-M1). Alternative — hand-rolled
JSON-RPC over `jsonrpsee`: full control, but we'd chase spec revisions (auth, streamable
HTTP, elicitation) forever. Fallback risk is low: the registry abstraction means swapping
protocol libraries touches one crate.

## 3. Exposed Surface

### Tools (verbs) — generated from `ToolSpec`s
Naming: `snake_case`, domain-grouped, mirroring CLI commands 1:1:

| Group | Tools (examples) |
|---|---|
| project | `create_project`, `open_project`, `get_project_summary`, `undo`, `snapshot` |
| compose | `generate_chords`, `generate_melody`, `generate_drums`, `generate_bass`, `fit_to_chords` |
| edit | `insert_clip`, `quantize`, `humanize`, `transpose`, `set_tempo` |
| mix | `set_level`, `add_device`, `set_param`, `route_send`, `analyze_mix` |
| render | `render_song`, `render_stems`, `analyze_audio`, `measure_loudness` |
| system | `list_plugins`, `doctor` |

Design rules for AI-quality tools (from MCP builder guidance + our research):
- Descriptions written for the model: when to use, constraints, cost hints ("render is
  slow; prefer analyze_mix for iteration").
- Inputs are strict schemas derived from Rust structs (`schemars`); enums over free strings.
- Outputs structured *and* summarized: `{ data: …, summary: "Added 8-bar chorus at bar 33" }`
  — models act on the summary, programs on the data.
- Idempotency tokens on mutating tools where retries are plausible.

### Resources (nouns, read-only)
`musicos://project/{id}/summary`, `…/tracks`, `…/pattern/{clip_id}`, `…/renders/{id}` —
lets hosts pull context without spending tool calls. Subscriptions (resource-updated
notifications) fed from the domain event bus.

### Prompts
Curated starters: `produce_song`, `remix_project`, `critique_mix` — encode our known-good
orchestration briefs so external hosts benefit from `06`'s prompt engineering.

## 4. Long-Running Operations

Renders and multi-stage compositions exceed sane request timeouts. Pattern:
1. Tool returns fast with `{ job_id }` after validation (`render_song` schedules on the job queue).
2. Progress via MCP progress notifications (bound to the request's progress token) and/or
   `musicos://job/{id}` resource polling.
3. `get_job_result` / cancellation tool (`cancel_job`) complete the lifecycle.

Same job queue the CLI uses (`--wait` makes CLI block and draw the progress bar; MCP
clients get notifications) — one implementation, two presentations.

## 5. Transports, Sessions, Security

- **stdio** (MUST): zero-config for Claude Code/Desktop; server binary spawned per client;
  project state shared through the project service's on-disk store + file locks.
- **Streamable HTTP/WebSocket** (SHOULD): `axum`-hosted for remote/desktop-companion use.
  Localhost binding by default; bearer token auth (generated per install) minimum; OAuth
  only if/when remote multi-user deployment becomes real (YAGNI now, socket design leaves room).
- **Safety model:** MCP callers get exactly the registry surface — which already means
  command validation, no filesystem/shell access, path-sandboxed asset references, and
  per-session rate/budget limits. Mutating tools honor a `--read-only` server mode
  (useful when pointing untrusted agent hosts at precious projects).

## 6. Versioning & Compatibility

- Tool schemas are semver'd with the SDK: additive changes freely; breaking input changes
  require a new tool name (`render_song_v2`) with the old one deprecated-but-alive for one
  minor cycle. Tool list is generated, so a `--schema-dump` CLI command emits the full
  surface for diffing in CI (catch accidental breakage).
- Protocol version negotiation handled by rmcp; we track the two latest MCP revisions.

## 7. Testing

- Conformance: MCP inspector run in CI against the stdio server (list/call/cancel golden flows).
- Contract tests: every registry tool must round-trip schema-validate its own examples
  (each `ToolSpec` carries ≥1 worked example — doubles as documentation and as few-shot
  material for `06`).
- Integration: scripted Claude Code session (live-AI nightly lane) executing S2 from `00` §5.

## 8. Future Evolution

- **Sampling** (server-initiated LLM calls through the client's model) could someday let
  MusicOS run reviewer critiques on the *host's* subscription instead of a configured
  provider — tracked, not v1.
- Elicitation for human-in-the-loop gates (`06` §9) once host support is widespread.
- A2A/agent-mesh protocols: whatever wins, it lands as another thin skin over the same registry.
