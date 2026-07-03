# Benchmarks

Criterion benchmark suites live inside the crates they measure
(`crates/<crate>/benches/`), per the methodology in
[`docs/11_Performance_Goals.md`](../docs/11_Performance_Goals.md) §4. This
directory holds cross-crate benchmark documentation and, later, tracked
results (`docs/11` §6).

## Running

```sh
cargo bench -p musicos-dsp          # one crate
cargo bench -p musicos-render -- --quick
just bench                          # everything (cargo bench --workspace)
```

Criterion writes HTML reports to `target/criterion/<bench>/report/index.html`.
Use `-- --quick` for a fast sanity pass; full runs give the numbers that count.

## Suites

| Bench target | Crate | What it measures | docs/11 budget it tracks |
|---|---|---|---|
| `processors` | `musicos-dsp` | `process()` cost of one 512-frame block for `BiquadStereo` (peak), `Compressor`, `StereoDelay`, `Reverb` | §2 callback deadline: per-node block cost feeds the 1.6 ms p99 budget @128/48k |
| `pattern` | `musicos-music-core` | `Pattern::quantized` / `Pattern::humanized` on 1000 notes; `Pattern::new` validation + sort on 10 000 shuffled notes | §3 command apply ≤ 1 ms (symbolic ops) |
| `smf` | `musicos-midi` | `export_smf` and `import_smf` on a 4-track × 2500-note song | §3 SMF import of a 10k-note file ≤ 50 ms |
| `render` | `musicos-render` | `compile_project` and end-to-end `render_project` of an 8-MIDI-track, 16-bar project with Delay + Reverb inserts | §3 graph compile ≤ 50 ms; offline render ≥ 10× real-time (REF-A), ≥ 4× (REF-CI) |

The `render` suite is the current stand-in for the full reference project
(docs/11 §1: 16 tracks, buses, sends, automation). It grows toward that spec
as the corresponding features land; until then it covers the MIDI half
(8 tracks, inserts, master sum).

## Regression policy

docs/11 §4: benches run on every PR; a > 5% regression on a tracked metric
fails review unless justified in the PR plus an ADR note. RT numbers cannot be
validated on CI runners — real-time budgets come from tracked runs on REF
hardware, with results and machine manifests checked in under
`benchmarks/results/`.

## Sample results

`cargo bench -p musicos-dsp -- --quick` — Apple M4, 2026-07-03:

| Bench | Time per 512-frame block |
|---|---|
| `dsp_block_512/biquad_peak` | ~1.78 µs |
| `dsp_block_512/compressor` | ~2.65 µs |
| `dsp_block_512/stereo_delay` | ~1.63 µs |
| `dsp_block_512/reverb` | ~9.0 µs |

For scale: a 512-frame block at 48 kHz is 10.67 ms of audio, so the heaviest
processor (reverb) costs ≈ 0.08% of real time per instance. Quick-mode numbers
are indicative only; use full runs for regression comparisons.
