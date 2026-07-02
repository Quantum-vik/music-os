# 05 — Music Core

Status: Draft v1 · Depends on: `03_Domain_Model.md` · Key ADRs: ADR-0004 (ticks), ADR-0008 (own theory crate)

The Music context: how MusicOS represents, imports, exports, analyzes, and transforms
symbolic music. Everything here is **pure, deterministic, dependency-light Rust** — it must
run in tests, on CI, in wasm, and under an LLM's tool call with identical results.

## 1. Representation Layers

```
Theory layer      PitchClass · Interval · Scale · Chord · Key           (harmony crate)
Pattern layer     Note · Pattern (sorted events, length in Ticks)        (music-core)
Project layer     Clip placements on Tracks over the TempoMap            (project-model, 03)
Interchange       SMF (midly) · MusicXML (subset) · JSON pattern dumps   (midi crate)
```

A `Pattern` is the workhorse: an immutable, tick-sorted collection of `Note`s (+ CC/pitch-bend
lanes) with a declared `length: Tick`. All composition tools produce/consume Patterns;
clips reference them (structural sharing → cheap duplication with variation, FR-C4).

## 2. Music Theory Engine (`harmony`, `rhythm`)

**Build our own (ADR-0008)** rather than wrapping existing crates (`rust-music-theory` et
al. are unmaintained/incomplete). Theory is core IP for an autonomous composer; we need:

- Pitch classes, intervals (compound, enharmonic-aware spelled pitches — `C#` ≠ `Db` for
  MusicXML round-trips, both map to `Pitch.note` for MIDI).
- Scales: church modes, harmonic/melodic minor, pentatonic, blues, bebop, custom sets.
- Chords: qualities, inversions, extensions/alterations, slash chords; `Chord ↔ Vec<Pitch>`
  voicing under constraints (range, doubling, spacing).
- Progressions: Roman-numeral analysis + functional grammar (T-PD-D), diatonic borrowing,
  secondary dominants. This grammar powers the rule-based composer and the *reviewer*
  agent's critique vocabulary (`06` §5).
- Voice-leading validation: parallel 5ths/8ves detection, spacing/crossing checks, cost
  function for smoothest voicing search (dynamic programming over voicing lattice).
- Rhythm: meter grids, syncopation measures, groove templates (swing %, per-slot offsets),
  Euclidean rhythms (game-changer for autonomous drum generation, trivial to implement).

## 3. Transformations (all pure `Pattern -> Pattern`)

| Operation | Notes |
|---|---|
| `quantize(grid, strength%, window)` | Partial quantize preserves feel; window excludes intentional pickups |
| `humanize(seed, timing_σ, velocity_σ, groove)` | **Seeded RNG only** (NFR-4); groove templates apply per-slot bias |
| `transpose(interval)` / `transpose_in(scale)` | Chromatic vs diatonic transposition |
| `invert`, `retrograde`, `augment(ratio)`, `diminish` | Counterpoint toolkit; ratios validated against PPQ divisibility |
| `merge`, `split_at`, `slice(range)`, `overlay` | Clip editing primitives |
| `legato`, `staccato(pct)`, `strum(spread)`, `arpeggiate(shape, rate)` | Performance renderers |
| `fit_to_chords(progression, policy)` | Re-pitch melody/bass to a new progression — the key tool for AI iteration loops |

Every transformation is a CLI command and an MCP tool via the tool registry (`02` §4) —
this catalogue is effectively the AI's instrument.

## 4. Generators (rule-based, v1 baseline)

Neural models are optional plugins (`09`); the deterministic baseline must be musically
credible on its own (NFR: works with zero models installed):

- **Chord progression generator:** functional-harmony Markov grammar per genre profile,
  seeded; outputs `Vec<(Chord, Duration)>`.
- **Melody generator:** constraint-based search over scale tones with contour templates,
  chord-tone targeting on strong beats, tension/resolution scoring.
- **Bass generator:** style profiles (root-fifth, walking, synth-ostinato) driven by the
  progression.
- **Drum generator:** per-genre pattern grammars + Euclidean variation + humanize.

Each implements the `Composer` trait (`composition` crate):

```rust
pub trait Composer: Send + Sync {
    fn describe(&self) -> ComposerSpec;                    // capabilities, params schema
    fn compose(&self, brief: &Brief, ctx: &MusicCtx, seed: Seed) -> Result<Pattern>;
}
```
`Brief` (key, tempo, genre tags, energy curve, references) is the shared contract between
AI planning and deterministic generation — the agent fills a Brief; the engine executes it.

## 5. Interchange Formats

- **SMF (MIDI files):** full import/export via `midly` (fast, well-maintained, zero-copy)
  wrapped entirely inside `crates/midi` (anti-corruption, `03` §7). Type 0 and 1; tempo
  and signature maps round-trip; PPQ conversion to 960 with residue preservation.
- **MusicXML (SHOULD, subset):** partwise import/export of notes, chords symbols, keys,
  meters. Needed for scored/orchestral workflows and research datasets. Full MusicXML is a
  tar pit — the supported subset is explicitly documented and tested against MuseScore
  exports. Own parser over `quick-xml`; no suitable Rust crate exists.
- **JSON pattern dump:** canonical serde form of `Pattern` — the format LLMs read/write in
  tool calls, and the golden-file format for snapshot tests. Compact note tuples
  `[start, dur, pitch, vel]` to be token-efficient (research finding: LLMs handle terse
  symbolic encodings like ABC better than verbose ones — `13` §3).
- **ABC notation (MAY):** cheap to add over the theory layer; strong LLM affinity
  (ChatMusician trained on ABC).

## 6. Analysis

`AnalyzeHarmony` (chords + key from a Pattern, HMM/template-matching over pitch-class
profiles), `AnalyzeRhythm` (density, syncopation, groove deviation), `AnalyzeForm`
(self-similarity matrix over clip content → section detection). Exposed as query tools —
these are the **eyes** of the reviewer agent; generation quality is bounded by the
system's ability to *hear symbolically*.

## 7. Testing Strategy (this crate family is the flagship for it)

- Property tests (proptest): `transpose(i) ∘ transpose(-i) == id`; quantize idempotence at
  strength 1.0; SMF round-trip `import(export(p)) == p`; voicing search respects constraints.
- Snapshot tests: generator outputs per (brief, seed) as JSON golden files — any diff is a
  reviewed musical decision, not an accident.
- Fuzzing: SMF and MusicXML parsers (`cargo-fuzz`) — parsers of untrusted files are the
  #1 memory-safety risk surface even in Rust (panics/DoS).
- Musical validity corpus: progressions/melodies checked against the voice-leading
  validator as a regression suite.

## 8. Alternatives Considered

| Decision | Alternative | Why rejected |
|---|---|---|
| Own theory crate | Wrap `rust-music-theory` | Unmaintained, no spelled pitches, no voice-leading; theory is core IP |
| `midly` for SMF | Own parser / `rimd` | midly is mature+fast; own parser is undifferentiated risk |
| Pattern immutability + sharing | Mutable note lists | Cheap snapshots/undo, safe concurrent reads by agents and UI |
| Rule-based baseline first | Neural-first generation | Determinism, CI-friendliness, and the research consensus (`13` §2): LLM-planned + tool-executed beats end-to-end generation for controllability |

## 9. Future Evolution

- Expression lanes / MPE on `Note` (reserved optional field, ADR note in 03 §9).
- Tuning systems beyond 12-TET layered over `PitchClass` without breaking `Pitch`.
- Neural composer plugins (ONNX) implementing the same `Composer` trait — briefs and seeds
  keep even stochastic models *reproducible per (model, seed)*.
- Score rendering (LilyPond/Verovio export) from the spelled-pitch layer.
