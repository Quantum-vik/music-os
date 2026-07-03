# Changelog

All notable changes to MusicOS are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
semantic versioning. Project format compatibility is governed separately by
`docs/08_Project_Format.md` (corpus-tested: old bundles open forever).

## [Unreleased]

Nothing yet.

## [0.1.1] - 2026-07-03

### Added
- Deterministic symbolic core: integer musical time (960 PPQ), seeded
  generators for chords/melody/bass/drums (functional-harmony grammar,
  Euclidean rhythms), pattern transforms, SMF import/export.
- Project model as command/event log with undo/redo, atomic `.musicos`
  bundle storage, cross-version format corpus.
- Offline renderer: compiled audio graph, built-in synth, EQ/compressor/
  delay/reverb inserts, per-track mix, byte-identical renders.
- Loudness engineering: BS.1770 integrated loudness measurement,
  loudness-targeted mastering with peak limiter (`music render --master`),
  `music analyze` for WAV files.
- Real-time playback: streaming engine with 32-voice pool, lock-free
  feeder/CPAL rings, block-boundary graph swap, `music play --from-bar`.
- Plugin system: `ProcessorPlugin` trait + conformance harness, native
  bitcrusher, CLAP hosting via dlopen (`music plugins --probe`), test CLAP
  plugin exercised end-to-end in CI.
- AI production agent over the tool registry: Claude subscription
  (Claude Code CLI) or Anthropic API, user-selectable (`music ai`).
- MCP stdio server (`music-server`) exposing every tool; Python SDK
  (`sdk/python`) speaking the same protocol.
- Desktop application (Tauri): control-surface UI (tracks/mix, generators,
  transport with live progress, render with mastering target, AI producer
  panel, raw tool console), start/stop playback sessions, installable
  bundles (.app/.dmg, .msi, .deb/.AppImage) published on release tags.
- Engineering: 3-OS CI (fmt, clippy -D warnings, nextest, doctests, layer
  check, rustdoc, audit/deny), criterion benchmarks with budget gates,
  fuzz-lite parser robustness tests, release workflow with checksums,
  mdBook docs site.
