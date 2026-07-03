//! Per-block cost of each insert processor (docs/11 §4: "DSP kernels —
//! per-block cost by node type"). One block is 512 frames at 48 kHz.

// Bench targets are not public API; `criterion_group!` emits an undocumented fn.
#![allow(missing_docs)]

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use musicos_dsp::{BiquadMode, BiquadStereo, Compressor, Reverb, StereoDelay};

const FRAMES: usize = 512;
const SAMPLE_RATE: u32 = 48_000;

/// A 440 Hz half-scale sine block, duplicated to both channels.
fn buffers() -> (Vec<f32>, Vec<f32>) {
    let left: Vec<f32> = (0..FRAMES)
        .map(|i| (core::f32::consts::TAU * 440.0 * i as f32 / SAMPLE_RATE as f32).sin() * 0.5)
        .collect();
    let right = left.clone();
    (left, right)
}

/// Benchmarks `process()` on one 512-frame block for every processor type.
fn bench_processors(c: &mut Criterion) {
    let mut group = c.benchmark_group("dsp_block_512");

    let mut eq = BiquadStereo::new(BiquadMode::Peak, SAMPLE_RATE, 1_000.0, 1.0, 6.0);
    let (mut l, mut r) = buffers();
    group.bench_function("biquad_peak", |b| {
        b.iter(|| eq.process(black_box(&mut l), black_box(&mut r)));
    });

    let mut comp = Compressor::new(SAMPLE_RATE, -20.0, 4.0, 5.0, 100.0, 3.0);
    let (mut l, mut r) = buffers();
    group.bench_function("compressor", |b| {
        b.iter(|| comp.process(black_box(&mut l), black_box(&mut r)));
    });

    let mut delay = StereoDelay::new(SAMPLE_RATE, 350.0, 0.4, 0.35);
    let (mut l, mut r) = buffers();
    group.bench_function("stereo_delay", |b| {
        b.iter(|| delay.process(black_box(&mut l), black_box(&mut r)));
    });

    let mut reverb = Reverb::new(SAMPLE_RATE, 0.7, 0.4, 0.3);
    let (mut l, mut r) = buffers();
    group.bench_function("reverb", |b| {
        b.iter(|| reverb.process(black_box(&mut l), black_box(&mut r)));
    });

    group.finish();
}

criterion_group!(benches, bench_processors);
criterion_main!(benches);
