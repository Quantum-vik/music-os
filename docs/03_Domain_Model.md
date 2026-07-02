# 03 — Domain Model

Status: Draft v1 · Depends on: `02_System_Architecture.md` · Key ADRs: ADR-0004 (time units), ADR-0003 (commands/events)

DDD applied pragmatically: we name bounded contexts, aggregates, value objects, commands,
and events precisely, because in an AI-driven system **the domain model *is* the API** —
every LLM tool schema and every project-file field derives from it.

## 1. Bounded Contexts

| Context | Owns | Crates | Talks to |
|---|---|---|---|
| **Music** (symbolic) | Notes, chords, scales, patterns, theory | `music-core`, `harmony`, `rhythm`, `midi` | none (pure) |
| **Project** | Project/Track/Clip aggregate, tempo map, automation, mix model | `project-model`, `timeline` | Music (holds patterns) |
| **Performance** | Audio graph, voices, buffers, transport | `audio-graph`, `audio-engine`, `dsp`, `instruments` | consumes *compiled* Project |
| **Production AI** | Agents, sessions, plans, budgets | `ai-runtime`, `ai-providers` | issues Commands to Project |
| **Workshop** | Plugins, presets, sample library | `plugin-api`, `plugin-host`, `storage` | referenced by Project |

Context boundaries are translation boundaries: e.g. the Performance context never sees a
`Note` — it sees a `ScheduledEvent { sample_offset, payload }` produced by compilation.
This is the anti-corruption layer that keeps the RT engine ignorant of music theory.

## 2. Core Value Objects (Music context)

All value objects are `Copy` where possible, total-ordered, and serde-serializable.

```rust
/// Musical time. 960 PPQ fixed. Integer — never floating point.
pub struct Tick(pub i64);                     // ADR-0004
pub struct PPQ;                               // const 960: divisible by 2..8, 12, 16, 32, 64

/// Pitch as MIDI note number + optional microtonal offset (cents).
pub struct Pitch { pub note: u8, pub cents: i16 }

pub struct Velocity(pub u8);                  // 0..=127, validated at construction
pub struct TimeSignature { pub numerator: u8, pub denominator: u8 }
pub struct Tempo { pub micros_per_quarter: u32 }   // integer, like SMF — exact

pub struct Note   { pub pitch: Pitch, pub velocity: Velocity, pub start: Tick, pub duration: Tick }
pub struct Chord  { pub root: PitchClass, pub quality: ChordQuality, pub extensions: … }
pub struct Scale  { pub tonic: PitchClass, pub kind: ScaleKind }   // church modes, melodic/harmonic minor, custom
pub struct Key    { pub scale: Scale }
```

**Why integer ticks (ADR-0004):** floats accumulate rounding across edits and break
determinism (NFR-4) and cross-platform reproducibility. 960 PPQ chosen over 480 (finer
tuplets) and over rational numbers (simplicity, cache-friendliness); tuplets that don't
divide 960 are approximated at import with recorded residue so re-export is lossless.
Wall-clock seconds are *derived* via the tempo map, never stored on notes.

**Why `Pitch` carries cents:** microtonality and humanization without a second model;
SMF export maps cents to pitch-bend where needed.

## 3. Aggregates (Project context)

```
Project (aggregate root)
├── meta: ProjectMeta { id, name, created, format_version }
├── tempo_map: TempoMap            // Vec<(Tick, Tempo)>, sorted — invariant TM1
├── signature_map: Vec<(Tick, TimeSignature)>
├── tracks: Vec<Track>
│     Track { id, name, kind: Midi|Audio|Bus, clips: Vec<ClipRef>,
│             mix: ChannelStrip, instrument: Option<DeviceRef> }
├── clips: SlotMap<ClipId, Clip>   // Clip = MidiClip(Pattern) | AudioClip(AssetRef, warp)
├── automation: Vec<AutomationLane>   // target: ParamAddr, curve: Vec<(Tick, f64, CurveShape)>
├── mix_graph: MixGraph            // tracks→buses→master; sends; invariant MG1: acyclic
└── markers, loop_region, assets
```

Invariants enforced *in the aggregate*, not in services (so no client can corrupt state):

- **TM1** tempo/signature maps sorted, unique ticks, entry at tick 0 always present.
- **CL1** clips within a track may overlap only if track policy allows (MIDI: yes, Audio: no).
- **MG1** mix graph is a DAG; sends cannot create cycles (checked on every routing command).
- **AU1** automation points sorted by tick; values within the parameter's declared range.
- **ID1** all references (`ClipRef`, `DeviceRef`, `AssetRef`) are typed newtype IDs; dangling
  references are impossible to construct through commands (validated) and detected on load.

Aggregate mutation is **only** through `apply(command) -> Result<Vec<Event>, DomainError>`.
State snapshots are cheap because collections use persistent/immutable structures (`im` crate
or Arc-cloned slotmaps — benchmark before choosing; ADR pending).

## 4. Commands and Events (the ubiquitous language)

Commands are imperative, validated, rejectable. Events are past-tense facts, the source of
undo and the AI's observation stream. Naming convention: `VerbNoun` / `NounVerbed`.

| Command (examples) | Emits |
|---|---|
| `CreateTrack { kind, name }` | `TrackCreated { track_id, … }` |
| `InsertClip { track, at, pattern }` | `ClipInserted` |
| `SetTempo { at, tempo }` | `TempoChanged` |
| `AddDevice { track, device, index }` | `DeviceAdded` |
| `SetParam { addr, value }` / `WriteAutomation { lane, points }` | `ParamSet` / `AutomationWritten` |
| `RouteSend { from, to, gain }` | `SendRouted` (or `DomainError::WouldCreateCycle`) |

Undo = inverse commands derived from events (each event knows its inverse); grouped into
`Transaction`s so one user/agent action undoes atomically.

**Why commands rather than direct setters:** (a) single choke point for validation and
AI safety (FR-AI4); (b) serializable → project log, replay, testing; (c) the tool registry
(`02` §4) maps 1:1 onto commands + queries, so the MCP surface falls out of the domain.

## 5. Queries (read side)

Typed, side-effect-free, also exposed as tools: `GetProjectSummary`, `GetTrack`,
`GetPatternNotes`, `AnalyzeHarmony`, `GetMixGraph`, `GetLoudness(render_ref)`. Reads are
served from the immutable `ProjectState` snapshot; heavier analyses run on worker threads.

## 6. Domain Services (pure logic that spans aggregates)

- `HarmonyService`: chord detection, scale suggestion, voice-leading validation (`harmony`).
- `QuantizeService` / `HumanizeService`: pure `Pattern -> Pattern` functions with explicit
  parameters (grid, strength, swing, seed). Humanize takes an explicit RNG seed — **all
  randomness in the domain is seeded** (NFR-4 determinism).
- `ArrangementService`: section operations over the timeline.
- `CompileService` (bridge to Performance): `ProjectState -> CompiledGraph + EventSchedule`,
  the one-way translation into the engine's world (`04` §5).

## 7. Anti-Corruption at Every Port

External representations never leak inward:
- SMF (via `midly`) ↔ internal `Pattern` translation lives in `crates/midi` only.
- Plugin parameter conventions (VST3 normalized floats, CLAP) map to `ParamAddr` + typed
  ranges in the host adapters.
- LLM output (JSON) is parsed into Commands by `ai-runtime`; a malformed plan is a rejected
  command, never a corrupted project.

## 8. Alternatives Considered

| Decision | Alternative | Why rejected |
|---|---|---|
| Integer ticks @960 PPQ | f64 beats (Tracktion-style) | Non-determinism, comparison hazards; DAW float bugs are folklore |
| Commands/events | Direct mutable API | No undo/audit/AI-safety for free; every client reimplements validation |
| SlotMap + typed ids | `Vec` indices / UUID strings everywhere | Indices unstable under deletion; stringly-typed ids defeat the type system (UUIDs kept only at persistence boundary) |
| Cents-based microtonality | Full ratio-based tuning model | YAGNI for v1; cents cover MPE/pitch-bend; ratio model can layer on later |

## 9. Future Evolution

- New clip kinds (video, generative) = new `Clip` variant + commands; storage format is
  versioned enums with `#[serde(other)]` unknown-tolerance (`08` §5).
- Per-track time (poly-tempo) is representable by moving `TempoMap` into `Track` later;
  APIs already take a track context to avoid breaking signatures.
- MPE/expressive MIDI: `Note` gains optional expression lanes; `Pitch.cents` already anticipates it.
