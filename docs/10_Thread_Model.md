# 10 — Thread Model

Status: Draft v1 · Depends on: `02`, `04` · Key ADRs: ADR-0015 (three worlds, message passing only)

## 1. The Three Worlds (ADR-0015)

```
┌─ ASYNC WORLD ──────────────┐  ┌─ WORKER WORLD ─────────────┐  ┌─ REAL-TIME WORLD ────────┐
│ tokio multi-thread runtime │  │ rayon pool + job queue      │  │ 1 audio thread (CPAL cb) │
│ · services & tool dispatch │  │ · offline render            │  │ · GraphExecutor          │
│ · AI sessions (network)    │  │ · graph compilation         │  │ · Transport/Scheduler    │
│ · MCP/RPC transports       │  │ · analysis, waveform cache  │  │ · VoiceManager           │
│ · event bus, config, log   │  │ · plugin scanning (subproc) │  │ (+ optional pinned RT    │
│                            │  │ · file/db I/O (spawn_block) │  │   helpers, post-v1)      │
└────────────┬───────────────┘  └───────────┬────────────────┘  └───────────┬──────────────┘
             │        tokio::mpsc / channels │        rtrb SPSC rings + Arc swaps
             └────────────────◀──────────────┴──────────────▶────────────────┘
```

**Rule zero: worlds share no locks.** All cross-world communication is message passing or
atomic pointer swaps of immutable data. This is the actor model applied only where it pays
rent (`02` §1) — inside each world, ordinary Rust ownership applies.

## 2. Who Runs Where (and why)

| Work | World | Rationale |
|---|---|---|
| Command validation & state apply | Async (a single `ProjectActor` task) | Serializes writes → no state locks; the actor *is* the transaction boundary |
| Reads/queries | Any (Arc snapshot) | `ProjectState` snapshots are immutable — lock-free reads by GUI/AI/CLI concurrently |
| LLM calls, MCP I/O | Async | Network-bound; tokio's home turf |
| Graph compile, render, analysis | Worker | CPU-bound; must not starve the async reactor (never >~100 µs of CPU on a tokio thread — enforced by convention + `tokio-console` in dev) |
| Audio callback | RT | See `04` §2 contract |
| Blocking file/db I/O | Worker via `spawn_blocking` | Keep the reactor responsive |

## 3. Channel Inventory (explicit, bounded, typed)

| Channel | Type | Direction | Capacity policy |
|---|---|---|---|
| Engine commands | `rtrb` SPSC, `Copy` enums | async → RT | fixed 256; **producer backpressure = command coalescing** (last-wins for param sets), never blocking the sender on RT |
| Engine events (meters, playhead, xruns) | `rtrb` SPSC, fixed-size records | RT → async | fixed 1024; overflow = drop-oldest meters (lossy OK), never block RT |
| Graph swap | `Arc<CompiledGraph>` via command slot | async → RT | old graph returned via event ring → dropped off-thread (`04` §2) |
| Param table | atomic f32 cells | any → RT read | wait-free; smoothing on RT side |
| Domain event bus | `tokio::sync::broadcast` | services → subscribers (GUI, AI, MCP notif) | 1024, lagging subscribers detect + resync via snapshot |
| Job queue | `tokio::mpsc` + worker pool | services → workers | bounded 64; `JobHandle` (watch channel) carries progress/cancel |

Design rules: every channel is **bounded** (unbounded queues hide overload until OOM);
every overflow has a *named policy* (coalesce, drop-lossy, backpressure, error); only
`Copy`/`Arc` payloads cross the RT boundary (no drops of heap data on RT).

## 4. RT Thread Contract (enforceable checklist)

On the audio callback thread it is forbidden to: allocate/free; lock any mutex; do I/O;
call `tracing`/format; touch `Arc::drop` of non-trivial data; call into tokio; park/sleep;
run unbounded loops. Enforcement is layered:
1. **Type discipline:** the executor takes `&CompiledGraph` (no `&mut` shared state) and a
   `RtScratch` of pre-allocated buffers; RT-facing APIs take `#[derive(Copy)]` messages.
2. **Debug allocator guard** (assert_no_alloc pattern) wrapping the callback in dev/test builds — allocation panics the test suite, not the gig (`11` §5).
3. **Clippy lint wall + code review gate** for `crates/audio-engine`, `dsp`, `instruments`:
   these crates forbid `std::sync::Mutex`, `println!`, `tracing` macros via lint config.
4. RT thread priority: request realtime scheduling class per-OS (`audio_thread_priority`
   crate pattern); document graceful degradation when denied (Linux without rtkit).

## 5. Cancellation, Shutdown, Panics

- Jobs: cooperative cancellation via `CancellationToken` checked at block boundaries
  (renders cancel within one block, ≤ ~3 ms of work lost).
- Shutdown order: stop transport → drain RT command ring → park audio stream → flush
  project log (fsync) → stop workers → stop runtime. Codified in one `Shutdown` sequence
  owned by the app shell, tested with a chaos test (SIGTERM at random points; the format's
  crash-safety guarantees, `08` §9, are the backstop).
- Panics: worker panics fail the job (reported as structured error); the `ProjectActor`
  panicking is a bug class we treat as fatal-with-clean-log (state is safe by
  construction: log append happens before ack). RT panic policy per `04` §10.

## 6. Async Discipline (Rust-specific decisions)

- **No `async` in domain crates.** Domain functions are sync + pure; only services and
  adapters are async. Keeps `music-core` wasm/embedded-friendly and trivially testable.
- No `block_on` inside the runtime; adapters that wrap sync SDKs use `spawn_blocking`.
- `Send + 'static` bounds on service APIs from day one (learned pain: retrofitting is misery).
- One tokio runtime per process, owned by the app shell — libraries never create runtimes
  (the classic embedded-runtime bug factory).

## 7. Alternatives Considered

| Decision | Alternative | Why rejected |
|---|---|---|
| Single writer `ProjectActor` | `RwLock<ProjectState>` | Writer starvation & lock-order risk; actor gives ordering, audit, and backpressure naturally |
| rtrb SPSC rings | crossbeam MPMC to RT | MPMC costs & wake semantics unnecessary; SPSC is provably wait-free, and we control topology |
| Bounded everything | Unbounded channels | Failure at the edge (visible, policied) beats OOM in the middle |
| Sync domain | Async trait domain | Infectious `async` complicates testing/wasm; zero domain operations are I/O-bound |

## 8. Future Evolution

- RT worker pool for parallel graph execution (`04` §3) — adds pinned threads to the RT
  world with a work-stealing *wait-free* handoff; the schedule format already encodes levels.
- Multiple concurrent projects (server mode): one `ProjectActor` + engine pair per project;
  worlds architecture unchanged (it's per-project already by construction).
- Remote workers (render farm): the job queue port grows a distributed adapter (`02` §8).
