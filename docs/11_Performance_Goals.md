# 11 — Performance Goals

Status: Draft v1 · Depends on: `04`, `10` · Numbers are commitments, not aspirations: each becomes a CI benchmark or a tracked measurement.

## 1. Reference Environments

| Tier | Hardware (representative) | Meaning |
|---|---|---|
| REF-A | Apple M1/M2, 16 GB | Primary target; all MUST numbers hold here |
| REF-B | 4-core x86 laptop (~2019, Win/Linux) | Numbers hold at 256-sample buffers |
| REF-CI | GitHub Actions runners | Offline/render/symbolic budgets only (no RT audio guarantees in CI) |

The **reference project** (checked into `assets/bench/`): 16 tracks (8 MIDI w/ built-in
synth+sampler, 8 audio), 4 buses, 12 insert effects, 2 sends, 4-minute arrangement,
automation on 32 params. All engine budgets are stated against it.

## 2. Real-Time Engine Budgets

| Metric | Target | How measured |
|---|---|---|
| Callback deadline @128/48k (2.67 ms) | p99 ≤ 60% of budget (1.6 ms); worst-case < 100% | RT-side timestamp histogram exported via event ring |
| Xruns | ≤ 1/hour steady-state on REF-A | `music doctor --monitor` |
| Allocation on RT thread | **0** | debug allocator guard (fails tests) |
| Locks on RT thread | **0** | code audit + lint wall (`10` §4) |
| Graph swap glitch | 0 dropped blocks on swap | integration test toggling graphs under playback |
| Param change → audible | ≤ 1 block + smoothing (~10 ms ramp) | test with recorded output |
| Voice count | ≥ 256 simultaneous built-in synth voices @128/48k on REF-A | criterion + RT soak test |

## 3. Offline & Symbolic Budgets

| Metric | Target |
|---|---|
| Offline render, reference project | ≥ 10× real-time REF-A; ≥ 4× REF-CI (NFR-3) |
| Project open (snapshot + 1k-command log tail) | ≤ 300 ms |
| Command apply → event emitted (p99) | ≤ 1 ms symbolic ops; ≤ 10 ms incl. log fsync |
| Graph compile, reference project | ≤ 50 ms (it gates edit-during-playback UX) |
| SMF import 10k-note file | ≤ 50 ms |
| CLI cold start (`music --help`) | ≤ 150 ms (NFR-5) |
| MCP tool round-trip (non-render, local) | ≤ 25 ms overhead over the underlying command |
| Memory: engine idle / reference project loaded | ≤ 150 MB / ≤ 600 MB (excl. sample data) |

AI-path budgets are *cost* budgets, not latency (network-dominated): per-stage token caps
and per-run wall-clock caps are config-enforced (`06` §4) with defaults documented there.

## 4. Methodology

- **criterion** micro/meso benches per crate (`benchmarks/`): DSP kernels (per-block cost
  by node type), graph executor, tick↔sample conversion, serde round-trips, log append.
- **Macro benches:** scripted CLI runs on the reference project (render, open, compose
  pipeline with MockModel) — measured in CI, tracked over time.
- **Regression policy:** benches run on every PR (Bencher/criterion-compare against main);
  > 5% regression on a tracked metric fails review unless justified in the PR + ADR note.
- **Profiling toolkit** documented in `scripts/`: `cargo flamegraph`, `perf`/Instruments,
  `tracy` feature flag for frame-style engine profiling, `tokio-console` for async health.
- Optimize in this order: **algorithm → memory layout → SIMD → parallelism**. No SIMD/
  parallel PRs without a benchmark demonstrating the win on REF hardware (this is how the
  parallel-graph decision in `04` §3 gets made).

## 5. Performance-Correctness Gates (tested invariants)

1. Debug allocator guard around the audio callback in every engine integration test.
2. Denormal soak test: reverb/filter tails run 10 min; CPU must not climb (FTZ working).
3. Determinism gate: render the reference project twice; hashes must match (per platform).
4. Overload behavior test: deliberately overloaded graph must trigger the fade-and-pause
   policy (`04` §10), not garbage output.
5. Long-session soak (nightly): 8-hour playback loop; RSS drift < 1%/hour, zero xrun burst trend.

## 6. Non-Goals / Honest Limits

- No cross-platform bit-identical *audio* (documented in `04` §6; symbolic state carries
  that guarantee instead).
- No latency guarantees under denied RT scheduling privileges (documented degradation).
- CI runners can't validate RT deadlines — RT numbers come from tracked runs on
  maintainers' REF hardware (results checked into `benchmarks/results/` with machine
  manifests), a deliberately low-tech but auditable process until dedicated bench hardware exists.
