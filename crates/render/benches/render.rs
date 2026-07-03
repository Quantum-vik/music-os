//! Reference-project render bench (docs/11 §1 and §3): graph compile ≤ 50 ms,
//! offline render ≥ 10x real-time on REF-A. The project approximates the
//! docs/11 reference project's MIDI half: 8 MIDI tracks, one 16-bar clip of
//! eighth notes each, with a Delay and a Reverb insert on the first two tracks.

// Bench targets are not public API; `criterion_group!` emits an undocumented fn.
#![allow(missing_docs)]

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use musicos_core_types::{Pitch, ProjectId, Tick, Velocity, PPQ};
use musicos_music_core::{Note, Pattern};
use musicos_project_model::{Command, Device, ProjectState, TrackKind};
use musicos_render::{compile_project, render_project, RenderOptions};

const TRACKS: i64 = 8;
const BARS: i64 = 16;

/// One 16-bar pattern of eighth notes (4/4), pitch varied per track.
fn clip_pattern(track: i64) -> Pattern {
    let eighth = PPQ / 2;
    let notes: Vec<Note> = (0..BARS * 8)
        .map(|i| Note {
            pitch: Pitch::new(40 + u8::try_from((track * 3 + i) % 36).expect("< 36")),
            velocity: Velocity::MF,
            start: Tick(i * eighth),
            duration: Tick(eighth),
        })
        .collect();
    Pattern::new(notes, Tick(BARS * 4 * PPQ)).expect("valid notes")
}

/// Builds the benchmark project via command dispatch, like a client would.
fn project() -> ProjectState {
    let mut state = ProjectState::new(ProjectId(1), "bench-reference");
    for t in 0..TRACKS {
        state
            .dispatch(Command::CreateTrack {
                name: format!("midi-{t}"),
                kind: TrackKind::Midi,
            })
            .expect("create track");
        let track = state.tracks.last().expect("track exists").id;
        state
            .dispatch(Command::InsertClip {
                track,
                name: format!("clip-{t}"),
                pattern: clip_pattern(t),
                at: Tick::ZERO,
            })
            .expect("insert clip");
    }
    let first = state.tracks[0].id;
    state
        .dispatch(Command::AddDevice {
            track: first,
            device: Device::Delay {
                time_ms: 350.0,
                feedback: 0.4,
                mix: 0.3,
            },
        })
        .expect("add delay");
    let second = state.tracks[1].id;
    state
        .dispatch(Command::AddDevice {
            track: second,
            device: Device::Reverb {
                room: 0.7,
                damping: 0.4,
                mix: 0.25,
            },
        })
        .expect("add reverb");
    state
}

/// Benchmarks graph compilation and the end-to-end offline render.
fn bench_render(c: &mut Criterion) {
    let state = project();
    let opts = RenderOptions::default();

    let mut group = c.benchmark_group("render_reference");
    group.sample_size(10);

    group.bench_function("compile_project", |b| {
        b.iter(|| compile_project(black_box(&state), black_box(&opts)).expect("compiles"));
    });

    group.bench_function("render_project", |b| {
        b.iter(|| render_project(black_box(&state), black_box(&opts)).expect("renders"));
    });

    group.finish();
}

criterion_group!(benches, bench_render);
criterion_main!(benches);
