# 04 — Audio Architecture

Status: Draft v1 · Depends on: `02`, `03`, `10_Thread_Model.md` · Key ADRs: ADR-0005 (graph compilation), ADR-0006 (CPAL), ADR-0007 (f32 pipeline)

## 1. Design Constraints (from prior art)

Studying Ardour, Reaper (public docs/behavior), JUCE/Tracktion Engine, and NIH-plug/CPAL
yields the constraints this design inherits (full notes in `13_Research.md` §5):

1. **The callback is a hard deadline.** At 128 samples / 48 kHz we get 2.67 ms per block —
   miss it and the user hears it. Everything below follows from this.
2. **Never do on the RT thread:** allocate, free, lock a contended mutex, do file/network
   I/O, park, spawn, log via formatting, touch reference-counted drops of big objects.
3. **Graph changes are rare; audio is continuous.** So: *compile* the graph off-thread,
   *swap* it atomically (Ardour's "rt-safe graph swap", JUCE's `AudioProcessorGraph`
   rebuild). ADR-0005.
4. **Offline and real-time rendering must share one code path** (Tracktion lesson) or they
   *will* diverge audibly. Same graph executor, different driver.

## 2. Two Worlds, One Engine

```
 CONTROL WORLD (tokio)                      REAL-TIME WORLD (audio thread)
┌───────────────────────────┐   SPSC ring  ┌──────────────────────────────┐
│ ProjectService            │──commands──▶ │ EngineActor                  │
│ CompileService            │  (rtrb)      │  ├ Transport (sample clock)  │
│  ProjectState             │              │  ├ GraphExecutor             │
│    └─▶ CompiledGraph ──── │──Arc swap──▶ │  ├ EventScheduler            │
│ meters/events UI, AI      │◀──events───  │  └ VoiceManager              │
└───────────────────────────┘  (rtrb)      └──────────────────────────────┘
```

- **Command queue** (control→RT): fixed-size lock-free SPSC ring (`rtrb`). Messages are
  small `Copy` enums: `Play`, `Stop`, `Seek(SamplePos)`, `SwapGraph(GraphHandle)`,
  `SetParam(ParamAddr, f32)`.
- **Event queue** (RT→control): meters, playhead, xrun reports, fixed-size records.
- **Graph swap:** new `CompiledGraph` is built and pre-allocated on a worker thread,
  delivered as an `Arc`; the RT thread swaps a pointer and pushes the old one back over
  the event queue so **deallocation happens off-thread** (classic Ardour/JUCE pattern).
- Parameters: each automatable param is an atomic f32 cell in a pre-allocated table;
  automation and live tweaks write cells, DSP reads + smooths them (per-sample ramp) to
  avoid zipper noise.

## 3. The Audio Graph

`audio-graph` models a DAG of nodes: `Source` (instrument, sampler, audio clip reader),
`Processor` (insert effects, plugin wrappers), `Sum` (bus), `Sink` (master out, render sink).

Compilation (`CompileService`, off-thread):
1. Flatten project mix model → node graph; verify acyclicity (domain invariant MG1).
2. **Topological sort** → linear schedule of `Task { node, in_bufs, out_bufs }`.
3. **Buffer allocation by liveness analysis:** buffers are reused once their last consumer
   ran (Reaper/Ardour-style pooling) — a 100-track project needs dozens, not hundreds, of
   block buffers. All buffers allocated at compile time, sized `max_block × channels`.
4. **Latency compensation (PDC):** each node reports latency; compiler computes per-path
   delay and inserts pre-allocated delay lines so parallel paths stay aligned. Reported
   plugin latency changes trigger recompilation, not RT-thread fixups.
5. Emit `EventSchedule`: all MIDI/automation events converted to sample positions using
   the tempo map (integer math from `Tick`, ADR-0004), sorted, stored in flat arrays for
   cache-friendly binary search per block.

**Parallel execution (post-v1):** schedule already carries dependency levels; a small
pinned RT worker pool can execute independent subtrees per block (Ardour "process graph").
v1 ships single-threaded RT execution — simpler, and profiling data should drive the
parallel design (see `11` §4).

### Alternatives considered

| Choice | Alternative | Why rejected |
|---|---|---|
| Compile+swap immutable graph | Mutate live graph under RT lock | Priority inversion & glitches; the folklore failure of naive engines |
| Liveness-based buffer pool | Buffer per edge | Memory blowup on large sessions |
| Sample-position event schedule | Evaluate tempo map per block on RT thread | More RT work, float drift; precomputation is deterministic and cheap |
| Single RT thread v1 | Parallel graph from day 1 | Complexity before evidence; design leaves the seam |

## 4. Transport & Clock

- Master clock = **sample counter** (u64) owned by the RT thread; musical time is derived
  in the control world for display. One source of truth, no drift.
- Transport states: `Stopped`, `Playing`, `Looping{start,end}`; seek/loop are messages;
  loop boundaries are sample-exact (loop region compiled into the event schedule).
- Real-time input MIDI (post-v1) timestamps at the driver callback and enters the RT world
  through its own SPSC ring.

## 5. Instruments, Voices, and Events

- `EventScheduler` slices each block: binary-search events in `[block_start, block_end)`,
  split processing at event boundaries (sample-accurate note-on, à la NIH-plug) rather
  than quantizing to block starts. Budget guard: max split points per block.
- `VoiceManager`: fixed-capacity voice pools per instrument (pre-allocated), voice stealing
  by (releasing > quietest > oldest) policy. No allocation at note-on.
- Built-in instruments (`instruments`): polyphonic subtractive synth (2 osc, SVF filter,
  2 env, 1 LFO) and a sampler (pre-loaded/streamed via off-thread prefetch ring). These
  exist to make the platform useful with zero plugins and to serve as reference
  implementations of `plugin-api`.

## 6. DSP Crate Principles

- Pure `process(ctx: &ProcessCtx, io: &mut BufferSet)` functions/structs; no allocation
  post-`prepare(sample_rate, max_block)`.
- **f32 internal pipeline (ADR-0007)**, f64 accumulators inside filters/summing where error
  analysis demands. Rationale: f32 is the plugin-ecosystem lingua franca and 2× SIMD width;
  Reaper's f64 path is a differentiator we don't need at the cost of halving throughput.
- Denormal protection: FTZ/DAZ set on the RT thread (`no_denormals` guard), plus DC-offset
  trick inside feedback structures for platforms without FTZ.
- Determinism note (NFR-4): renders are bit-identical **per platform + version**; we do not
  promise cross-platform bit-identity for audio (FMA/libm differences) — symbolic state is
  the cross-platform contract. Documented honestly in `08`.
- SIMD via `std::simd`-style abstractions behind our own `dsp::simd` shim so stable-Rust
  fallback (auto-vectorized scalar) always exists.

## 7. Offline Render

Same `GraphExecutor`, driven by `RenderDriver` instead of the device callback: a worker
thread loop pulls blocks as fast as possible (≥10× RT target, NFR-3), pipes into encoder
adapters (`hound` WAV, `claxon`-verified FLAC via encoder lib, mp3 via external/optional
feature), applies loudness normalization pass (two-pass EBU R128 via `ebur128` port).
Stems = one pass with multiple render sinks tapped at track/bus outputs.
Progress reported as events → CLI progress bar / MCP progress notification (same stream).

## 8. Audio I/O Port

```rust
pub trait AudioOutput {          // port in domain terms; CPAL is one adapter
    fn spec(&self) -> DeviceSpec;                       // rate, channels, buffer sizes
    fn start(&mut self, callback: RtCallback) -> Result<RunningStream>;
}
```
**CPAL chosen (ADR-0006)** as the v1 adapter: pure-Rust, all three platforms
(WASAPI/CoreAudio/ALSA). Known tradeoffs: no duplex guarantees, no ASIO by default, jack
support behind features — acceptable because v1 is playback/render-first (FR non-goal:
recording). The port keeps the door open for dedicated ASIO/JACK/PipeWire adapters.

## 9. Plugin Hosting Interaction

Plugins run *inside* the RT schedule as `Processor` nodes wrapped by host adapters
(`09_Plugin_System.md`). The host adapter is responsible for enforcing our RT contract at
the boundary (prepare/process split, no allocation in process). Sandboxed (out-of-process)
plugins get a shared-memory ring + one block of added latency, compensated by PDC — the
engine treats them as just another latency-reporting node.

## 10. Failure & Overload Behavior

- **Xrun policy:** detect (callback deadline miss via timestamp), count, report event;
  after N consecutive overloads, engine auto-fades and pauses rather than emitting garbage
  (protect ears and speakers). CLI `music doctor` surfaces xrun stats.
- **Panic policy:** RT thread code is `panic = abort`-audited; plugin adapter wraps FFI
  calls with `catch_unwind` where safe; sandboxed mode fully isolates crashes (NFR-7).
- Debug builds run the RT thread under an **allocation-detecting allocator guard**
  (assert_no_alloc pattern) so violations fail tests, not gigs (`11` §5).
