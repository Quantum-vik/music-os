//! Symbolic-op cost: pattern transformations and construction sorting
//! (docs/11 §3: command apply ≤ 1 ms budget rides on these primitives).

// Bench targets are not public API; `criterion_group!` emits an undocumented fn.
#![allow(missing_docs)]

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use musicos_core_types::{Pitch, Seed, Tick, Velocity, PPQ};
use musicos_music_core::rng::SplitMix64;
use musicos_music_core::{Note, Pattern};

fn note(start: i64, pitch: u8) -> Note {
    Note {
        pitch: Pitch::new(pitch),
        velocity: Velocity::MF,
        start: Tick(start),
        duration: Tick(PPQ / 4),
    }
}

/// A deterministic pattern of `n` slightly off-grid sixteenth notes.
fn pattern_of(n: i64) -> Pattern {
    let notes: Vec<Note> = (0..n)
        .map(|i| {
            note(
                i * (PPQ / 4) + (i % 7),
                48 + u8::try_from(i % 24).expect("< 24"),
            )
        })
        .collect();
    Pattern::new(notes, Tick(n * (PPQ / 4) + PPQ)).expect("valid notes")
}

/// Benchmarks `quantized` and `humanized` on a 1000-note pattern, and
/// `Pattern::new` (validation + sort) on 10 000 shuffled notes.
fn bench_pattern(c: &mut Criterion) {
    let p = pattern_of(1000);

    c.bench_function("pattern_quantized_1000", |b| {
        b.iter(|| black_box(&p).quantized(Tick(PPQ / 4), 60));
    });

    c.bench_function("pattern_humanized_1000", |b| {
        b.iter(|| black_box(&p).humanized(Seed(42), Tick(30), 10));
    });

    // 10k valid notes, deterministically shuffled so the sort does real work.
    let mut notes: Vec<Note> = (0..10_000)
        .map(|i| note(i * (PPQ / 8), 36 + u8::try_from(i % 48).expect("< 48")))
        .collect();
    let mut rng = SplitMix64::new(Seed(7));
    for i in (1..notes.len()).rev() {
        notes.swap(i, rng.index(i + 1));
    }
    c.bench_function("pattern_new_10000_shuffled", |b| {
        b.iter_batched(
            || notes.clone(),
            |n| Pattern::new(n, Tick::ZERO).expect("valid notes"),
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(benches, bench_pattern);
criterion_main!(benches);
