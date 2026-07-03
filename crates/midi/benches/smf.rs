//! SMF import/export cost on a 10k-note song (docs/11 §3: SMF import of a
//! 10k-note file ≤ 50 ms). Four tracks of 2500 notes each.

// Bench targets are not public API; `criterion_group!` emits an undocumented fn.
#![allow(missing_docs)]

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use musicos_core_types::{Pitch, Tempo, Tick, Velocity, PPQ};
use musicos_midi::{export_smf, import_smf, SmfSong};
use musicos_music_core::{Note, Pattern};
use musicos_timeline::TempoMap;

const TRACKS: i64 = 4;
const NOTES_PER_TRACK: i64 = 2500;

/// Builds a 4-track, 10k-note song of non-overlapping sixteenth notes.
fn song() -> SmfSong {
    let tracks = (0..TRACKS)
        .map(|t| {
            let notes: Vec<Note> = (0..NOTES_PER_TRACK)
                .map(|i| Note {
                    pitch: Pitch::new(36 + u8::try_from((i + t * 5) % 48).expect("< 48")),
                    velocity: Velocity::MF,
                    start: Tick(i * (PPQ / 4)),
                    duration: Tick(PPQ / 8),
                })
                .collect();
            let pattern = Pattern::new(notes, Tick::ZERO).expect("valid notes");
            (Some(format!("track-{t}")), pattern)
        })
        .collect();
    SmfSong {
        tempo_map: TempoMap::constant(Tempo::DEFAULT),
        tracks,
    }
}

/// Benchmarks `export_smf` on the song and `import_smf` on its bytes.
fn bench_smf(c: &mut Criterion) {
    let song = song();
    let bytes = export_smf(&song);

    c.bench_function("smf_export_4x2500", |b| {
        b.iter(|| export_smf(black_box(&song)));
    });

    c.bench_function("smf_import_4x2500", |b| {
        b.iter(|| import_smf(black_box(&bytes)).expect("valid SMF"));
    });
}

criterion_group!(benches, bench_smf);
criterion_main!(benches);
