# 06 — AI Runtime

Status: Draft v1 · Depends on: `02`, `03`, `05`, `07` · Key ADRs: ADR-0009 (Claude Agent SDK primary), ADR-0010 (AI acts via commands only)

## 1. The Prime Directive

**AI plans; services execute.** (ADR-0010)

```
LLM/Agent ──proposes──▶ ToolCall(JSON) ──validated──▶ Command ──▶ ProjectService ──▶ Events
                                                        ▲                              │
                                                        └────── observations ◀─────────┘
```

The AI layer has *no* privileged access: it uses the same tool registry (`02` §4) as the
CLI and MCP clients. Consequences:
- A hallucinated tool call is a **rejected command**, never corrupted state.
- Every AI action is in the project command log → auditable, undoable, replayable (NFR-4).
- Providers are swappable because the contract is "emit tool calls against these schemas,"
  which every serious LLM API supports.

## 2. Provider Abstraction (ports)

```rust
pub trait LanguageModel: Send + Sync {            // one-shot / streaming completion + tools
    fn capabilities(&self) -> ModelCaps;          // tools? streaming? context? vision?
    async fn generate(&self, req: GenerateRequest) -> Result<GenerateStream, ModelError>;
}
pub trait AgentSession: Send + Sync {             // stateful, multi-turn, tool-loop runtime
    async fn send(&mut self, turn: Turn) -> Result<TurnStream, ModelError>;
    fn id(&self) -> SessionId;
    async fn persist(&self) -> Result<SessionSnapshot>;
}
pub trait Embedder: Send + Sync { async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>; }
```

Two ports, deliberately: `LanguageModel` for stateless calls (classification, critique),
`AgentSession` for the tool-calling loop. The **Claude Agent SDK adapter implements
`AgentSession` natively** (the SDK runs the loop, sessions, retries); for raw-API providers
(OpenAI/Gemini/ONNX/llama.cpp) a generic `ToolLoopSession` in `ai-runtime` wraps their
`LanguageModel` impl. Domain logic never knows which path served it.

## 3. Claude Integration (ADR-0009)

Primary adapter: **Claude Agent SDK**, spoken to via the local Claude Code runtime.

- **Auth:** user subscription through the installed `claude` login (no key handling by us);
  fallback `ANTHROPIC_API_KEY`. Keys, when used, come from OS keychain/env via `config`.
- **Streaming:** SDK stream events mapped to our `TurnStream` (text deltas, tool-call
  starts/results, thinking summaries) → surfaced as CLI live output / MCP progress.
- **Tool calling:** we register the tool registry's `ToolSpec`s as SDK custom tools; the
  SDK invokes our async callbacks, which dispatch Commands. We do **not** grant the SDK's
  own file/bash tools — MusicOS tools only (least privilege).
- **Session management:** SDK session ids stored per project (`.musicos/sessions/`);
  `music compose --continue` resumes context.
- **Retry/error recovery:** adapter maps SDK errors to `ModelError { kind, retryable }`;
  `ai-runtime` owns policy: exponential backoff w/ jitter on retryable, circuit-breaker per
  provider, fallback provider chain (configurable), and *degrade to rule-based composer*
  as the terminal fallback so the pipeline never hard-fails on provider outage (NFR-7).

Why the Agent SDK over raw Messages API: session persistence, tool-loop, retries, and
subscription auth come for free and match FR-AI1 exactly. Tradeoff: heavier dependency and
a subprocess runtime; contained because it is *one adapter crate* — the raw-API adapter
path exists for servers/CI where the Claude runtime isn't installed.

## 4. Agent Topology

Findings from ComposerX/CoComposer/ByteComposer (`13` §2) — role-specialized agents with a
critique loop measurably outperform single-shot generation — shape this topology:

```
                    Orchestrator (planner)
                    · parses user brief → ProductionPlan (typed, validated)
                    · dispatches stages, owns budget & convergence
   ┌───────────┬──────────┼────────────┬─────────────┐
   Composer    Arranger   SoundDesign  Mixer         Mastering
   (briefs →   (sections, (instrument/ (levels, EQ,  (loudness,
   patterns)   energy)    preset sel.) sends, dyn.)  limiting)
                          Reviewer (critic)
                          · reads analysis tools (05 §6, mix analysis)
                          · scores against plan; emits revision requests
```

- Agents are **roles, not processes**: an agent = system prompt + allowed-tool subset +
  budget, executed on an `AgentSession`. Cheap to add/modify; config-defined in TOML.
- **Generate → Review → Revise** loop with hard bounds: max N revisions per stage, token
  and wall-clock budgets per stage and per run (runaway-agent protection). Convergence =
  reviewer score ≥ threshold or budget exhausted (take best-so-far, report honestly).
- Orchestrator output is a **typed `ProductionPlan`** (serde-validated), not free text —
  malformed plans are re-prompted with the parse error (self-repair), never executed.

Why not a general multi-agent framework (AutoGen-style free conversation): unbounded
token burn and non-reproducibility. Our workflow engine executes a *plan DAG* with agent
nodes — closer to HuggingGPT's plan-then-execute than to open-ended chat (`13` §4).

## 5. Context Management (what the model sees)

- **Project summarization:** never dump raw project JSON; `GetProjectSummary` produces a
  compact, LLM-oriented digest (tracks, sections, keys, tempo, device chains) — token cost
  is an engineering budget like any other.
- Patterns exchanged in the compact JSON tuple form (`05` §5).
- **Session memory** = conversation persistence (per provider). **Long-term memory**
  (post-v1) = embeddings of briefs/critiques/user preferences via `Embedder` port into a
  `VectorStore` port (Qdrant/LanceDB adapters) for "sounds like what you liked last time."

## 6. Determinism & Reproducibility Boundary

LLM output is inherently stochastic. The line we hold:
- Every AI run records: provider, model id, prompts, tool calls, seeds handed to
  generators, plan versions → stored in the project log (`08` §4) as an `AiRunRecord`.
- **Replaying the command log reproduces the project exactly without any model calls.**
  Model calls are only needed to produce *new* decisions. This is the property that keeps
  an AI-driven system debuggable (and CI-testable with a `MockModel` provider).

## 7. Evaluation & Testing

- `MockModel` / `ReplayModel` providers (fixture-driven) make the entire agent runtime unit-testable offline.
- Golden-plan tests: brief → plan snapshots with MockModel.
- Live smoke tests (opt-in, `--features live-ai`, nightly CI lane) against real Claude.
- Reviewer-agent metrics logged per run (score trajectories) → the data needed to tune
  prompts/loops later; telemetry is opt-in and local (NFR-10).

## 8. Alternatives Considered

| Decision | Alternative | Why rejected |
|---|---|---|
| Claude Agent SDK primary | Raw Messages API only | Reimplementing sessions/retries/tool-loop; loses subscription auth (FR-AI1) |
| Typed plan DAG orchestration | Free-form multi-agent chat (AutoGen/CAMEL style) | Unbounded cost, poor reproducibility, hard to test |
| AI-via-commands only | AI writes project files / holds mutable refs | Corruption risk, no audit, provider lock-in of behavior |
| Roles-as-config | Hardcoded agent structs per role | New roles shouldn't need code changes; prompts iterate faster than binaries |

## 9. Future Evolution

- Additional providers = new adapter crates only (open/closed).
- Local models: ONNX Runtime adapter (classification/embedding first), llama.cpp adapter
  behind `LanguageModel` for offline planning — capability-gated by `ModelCaps`.
- Multi-agent parallelism (compose sections concurrently) — plan DAG already expresses it;
  needs budget arbitration only.
- Human-in-the-loop checkpoints: plan pauses at configurable gates for approval (MCP
  elicitation / CLI prompt) — the workflow engine models this as a `WaitForApproval` node.
