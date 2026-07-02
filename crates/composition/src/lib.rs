//! Composer traits and rule-based composition engines.
//!
//! The deterministic baseline composers from `docs/05` §4: musically credible
//! with zero neural models installed, reproducible per `(input, seed)`
//! (NFR-4). **Structure drives everything**: chord progressions come from a
//! functional-harmony grammar (tonic → pre-dominant → dominant → tonic), and
//! melody/bass follow a given progression — chord tones on strong beats,
//! scale steps between (`docs/13` §3: controllability comes from structured
//! attributes, not prose).
//!
//! All generators are pure functions `inputs → Pattern`; neural composers
//! join later behind the same seams as plugins (`docs/09`).

use musicos_core_types::{Pitch, Seed, Tick, Velocity, PPQ};
use musicos_harmony::{Chord, ChordQuality, PitchClass, Scale};
use musicos_music_core::{rng::SplitMix64, Note, Pattern};

/// One bar per chord, 4/4 throughout the v1 generators.
pub const BAR: i64 = PPQ * 4;

/// Generates a chord progression with functional-harmony structure.
///
/// Grammar over scale degrees: tonic {I, vi} → pre-dominant {ii, IV} →
/// dominant {V, vii°} → tonic, starting on I and cadencing V → I. Minor keys
/// use the natural-minor diatonic triads with a major dominant (harmonic
/// practice). Same `(scale, bars, seed)` always yields the same progression.
pub fn generate_chords(scale: Scale, bars: usize, seed: Seed) -> Vec<Chord> {
    let mut rng = SplitMix64::new(seed);
    let degrees = diatonic_triads(scale);

    // Functional states as degree indices (0-based): T {0, 5}, PD {1, 3}, D {4}.
    let tonic: &[usize] = &[0, 5];
    let pre: &[usize] = &[1, 3];
    let dom: &[usize] = &[4];

    let mut out = Vec::with_capacity(bars.max(1));
    out.push(degrees[0]); // start on I
    let mut state = 0u8; // 0 = tonic, 1 = pre-dominant, 2 = dominant
    while out.len() < bars.max(1) {
        let remaining = bars - out.len();
        // Force the cadence: second-to-last bar dominant, last bar tonic.
        let degree = if remaining == 1 {
            degrees[0]
        } else if remaining == 2 {
            state = 0;
            degrees[dom[rng.index(dom.len())]]
        } else {
            match state {
                0 => {
                    // From tonic: move to pre-dominant (60%) or stay tonic.
                    if rng.chance(60) {
                        state = 1;
                        degrees[pre[rng.index(pre.len())]]
                    } else {
                        degrees[tonic[rng.index(tonic.len())]]
                    }
                }
                1 => {
                    // From pre-dominant: to dominant (70%) or another pre-dominant.
                    if rng.chance(70) {
                        state = 2;
                        degrees[dom[rng.index(dom.len())]]
                    } else {
                        degrees[pre[rng.index(pre.len())]]
                    }
                }
                _ => {
                    // From dominant: resolve to tonic.
                    state = 0;
                    degrees[tonic[rng.index(tonic.len())]]
                }
            }
        };
        out.push(degree);
    }
    out
}

fn is_minor(scale: Scale) -> bool {
    scale.kind.intervals().get(2).copied() == Some(3)
}

/// The seven diatonic triads of a (heptatonic) scale, with a major dominant
/// forced in minor keys. Pentatonics borrow the parent major/minor triads.
fn diatonic_triads(scale: Scale) -> Vec<Chord> {
    let steps = scale.kind.intervals();
    let heptatonic: Vec<u8> = if steps.len() == 7 {
        steps.to_vec()
    } else if is_minor(scale) {
        vec![0, 2, 3, 5, 7, 8, 10]
    } else {
        vec![0, 2, 4, 5, 7, 9, 11]
    };
    (0..7)
        .map(|degree| {
            let root = scale
                .tonic
                .transposed(i8::try_from(heptatonic[degree]).expect("interval < 12"));
            let third = (heptatonic[(degree + 2) % 7] + 12 - heptatonic[degree]) % 12;
            let fifth = (heptatonic[(degree + 4) % 7] + 12 - heptatonic[degree]) % 12;
            let quality = match (third, fifth) {
                (3, 7) => ChordQuality::Minor,
                (3, 6) => ChordQuality::Diminished,
                (4, 8) => ChordQuality::Augmented,
                _ => ChordQuality::Major, // (4, 7) and any exotic remainder
            };
            // Harmonic-practice dominant: force V major in minor keys.
            if degree == 4 && is_minor(scale) {
                Chord {
                    root,
                    quality: ChordQuality::Major,
                }
            } else {
                Chord { root, quality }
            }
        })
        .collect()
}

/// Renders a progression as sustained block chords, one bar each.
pub fn chords_to_pattern(progression: &[Chord], octave: i8, velocity: u8) -> Pattern {
    let vel = Velocity::clamped(i32::from(velocity));
    let mut notes = Vec::new();
    for (bar, chord) in progression.iter().enumerate() {
        let start = Tick(i64::try_from(bar).expect("bar count fits i64") * BAR);
        for pitch in chord.pitches(octave) {
            notes.push(Note {
                pitch,
                velocity: vel,
                start,
                duration: Tick(BAR),
            });
        }
    }
    Pattern::new(
        notes,
        Tick(i64::try_from(progression.len()).expect("fits") * BAR),
    )
    .expect("valid by construction")
}

/// Generates a bassline following the progression: root on beat 1, then a
/// seeded walk over root/fifth/octave on beats 3 (always) and 2/4 (sometimes).
pub fn generate_bass(progression: &[Chord], seed: Seed) -> Pattern {
    let mut rng = SplitMix64::new(seed);
    let mut notes = Vec::new();
    for (bar, chord) in progression.iter().enumerate() {
        let bar_start = i64::try_from(bar).expect("bar count fits i64") * BAR;
        let root = chord.root.in_octave(2);
        let choices = [0i16, 7, 12, -12]; // unison, fifth, octave up/down
        push_bass(&mut notes, root, 0, bar_start, PPQ * 2);
        if rng.chance(35) {
            push_bass(
                &mut notes,
                root,
                choices[rng.index(4)],
                bar_start + PPQ,
                PPQ,
            );
        }
        push_bass(
            &mut notes,
            root,
            choices[rng.index(4)],
            bar_start + PPQ * 2,
            PPQ * 2,
        );
        if rng.chance(45) {
            push_bass(
                &mut notes,
                root,
                choices[rng.index(4)],
                bar_start + PPQ * 3,
                PPQ,
            );
        }
    }
    Pattern::new(
        notes,
        Tick(i64::try_from(progression.len()).expect("fits") * BAR),
    )
    .expect("valid by construction")
}

fn push_bass(notes: &mut Vec<Note>, root: Pitch, offset: i16, start: i64, dur: i64) {
    let note = (i16::from(root.note) + offset).clamp(24, 60);
    notes.push(Note {
        pitch: Pitch::new(u8::try_from(note).expect("clamped")),
        velocity: Velocity::clamped(96),
        start: Tick(start),
        duration: Tick(dur),
    });
}

/// Generates a melody over the progression: chord tones on strong beats
/// (1 and 3), scale steps between, contour bounded to a singable octave-and-a-
/// half, seeded rhythm from eighth/quarter templates.
pub fn generate_melody(progression: &[Chord], scale: Scale, seed: Seed) -> Pattern {
    let mut rng = SplitMix64::new(seed);
    let scale_pcs = scale.pitch_classes();
    let mut notes = Vec::new();
    let mut current: i16 = 72; // start around C5

    // Per-bar rhythm templates: (start offset in eighths, length in eighths).
    let templates: [&[(i64, i64)]; 4] = [
        &[(0, 2), (2, 2), (4, 2), (6, 2)],
        &[(0, 2), (2, 1), (3, 1), (4, 4)],
        &[(0, 4), (4, 2), (6, 2)],
        &[(0, 1), (1, 1), (2, 2), (4, 2), (6, 1), (7, 1)],
    ];
    let eighth = PPQ / 2;

    for (bar, chord) in progression.iter().enumerate() {
        let bar_start = i64::try_from(bar).expect("bar count fits i64") * BAR;
        let template = templates[rng.index(templates.len())];
        for &(off, len) in template {
            let start = bar_start + off * eighth;
            let strong = off == 0 || off == 4; // beats 1 and 3
            let target_pcs: Vec<PitchClass> = if strong {
                chord
                    .quality
                    .intervals()
                    .iter()
                    .map(|&i| {
                        chord
                            .root
                            .transposed(i8::try_from(i).expect("interval < 12"))
                    })
                    .collect()
            } else {
                scale_pcs.clone()
            };
            current = nearest_in_pcs(
                (current + i16::try_from(rng.in_range(3)).expect("small")).clamp(62, 82),
                &target_pcs,
            );
            // Octave shifts keep the pitch class, so range correction never
            // knocks a note off its chord/scale target.
            while current < 60 {
                current += 12;
            }
            while current > 84 {
                current -= 12;
            }
            notes.push(Note {
                pitch: Pitch::new(u8::try_from(current).expect("clamped 60..=84")),
                velocity: Velocity::clamped(88 + i32::try_from(rng.in_range(12)).expect("small")),
                start: Tick(start),
                duration: Tick(len * eighth),
            });
        }
    }
    Pattern::new(
        notes,
        Tick(i64::try_from(progression.len()).expect("fits") * BAR),
    )
    .expect("valid by construction")
}

/// Snaps a pitch to the nearest pitch whose class is in `pcs`.
fn nearest_in_pcs(pitch: i16, pcs: &[PitchClass]) -> i16 {
    (0..=6)
        .flat_map(|d| [pitch - d, pitch + d])
        .find(|p| {
            let pc = u8::try_from(p.rem_euclid(12)).expect("mod 12");
            pcs.iter().any(|c| c.index() == pc)
        })
        .unwrap_or(pitch)
}

/// Drum styles for [`generate_drums`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DrumStyle {
    /// Kick 1 & 3, snare 2 & 4, eighth hats.
    Basic,
    /// Kick every beat, offbeat hats.
    FourOnFloor,
    /// Sparse, swung hats and ghost snares.
    LoFi,
}

impl DrumStyle {
    /// Parses a style name.
    ///
    /// # Errors
    /// Returns the unknown name for unknown styles.
    pub fn parse(name: &str) -> Result<DrumStyle, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "basic" | "rock" => Ok(DrumStyle::Basic),
            "four_on_floor" | "house" => Ok(DrumStyle::FourOnFloor),
            "lofi" | "lo-fi" => Ok(DrumStyle::LoFi),
            other => Err(other.to_string()),
        }
    }
}

/// General MIDI drum keys used by the generator.
pub const KICK: u8 = 36;
/// GM snare.
pub const SNARE: u8 = 38;
/// GM closed hi-hat.
pub const HAT: u8 = 42;

/// Generates a drum pattern: style skeleton plus seeded hat-velocity motion
/// and occasional ghost notes.
pub fn generate_drums(bars: usize, style: DrumStyle, seed: Seed) -> Pattern {
    let mut rng = SplitMix64::new(seed);
    let mut notes = Vec::new();
    let eighth = PPQ / 2;
    let hit = |notes: &mut Vec<Note>, key: u8, start: i64, vel: i32| {
        notes.push(Note {
            pitch: Pitch::new(key),
            velocity: Velocity::clamped(vel),
            start: Tick(start),
            duration: Tick(eighth / 2),
        });
    };

    for bar in 0..bars.max(1) {
        let s = i64::try_from(bar).expect("bar count fits i64") * BAR;
        match style {
            DrumStyle::Basic => {
                hit(&mut notes, KICK, s, 110);
                hit(&mut notes, KICK, s + PPQ * 2, 104);
                hit(&mut notes, SNARE, s + PPQ, 102);
                hit(&mut notes, SNARE, s + PPQ * 3, 102);
                for e in 0..8 {
                    hit(
                        &mut notes,
                        HAT,
                        s + e * eighth,
                        62 + i32::from(e % 2 == 0) * 16,
                    );
                }
            }
            DrumStyle::FourOnFloor => {
                for beat in 0..4 {
                    hit(&mut notes, KICK, s + beat * PPQ, 112);
                    hit(&mut notes, HAT, s + beat * PPQ + eighth, 84);
                }
                hit(&mut notes, SNARE, s + PPQ, 98);
                hit(&mut notes, SNARE, s + PPQ * 3, 98);
            }
            DrumStyle::LoFi => {
                hit(&mut notes, KICK, s, 100);
                if rng.chance(60) {
                    hit(&mut notes, KICK, s + PPQ * 2 + eighth, 88);
                }
                hit(&mut notes, SNARE, s + PPQ, 92);
                hit(&mut notes, SNARE, s + PPQ * 3, 92);
                for e in 0..8 {
                    // Swing: delay offbeat eighths; seeded velocity motion.
                    let swing = if e % 2 == 1 { eighth / 6 } else { 0 };
                    let vel = 48 + i32::try_from(rng.in_range(14)).expect("small");
                    hit(&mut notes, HAT, s + e * eighth + swing, vel);
                }
                if rng.chance(30) {
                    hit(&mut notes, SNARE, s + PPQ * 2 + eighth + eighth / 2, 40);
                }
            }
        }
    }
    Pattern::new(notes, Tick(i64::try_from(bars.max(1)).expect("fits") * BAR))
        .expect("valid by construction")
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicos_harmony::ScaleKind;

    fn c_major() -> Scale {
        Scale {
            tonic: PitchClass::new(0),
            kind: ScaleKind::Major,
        }
    }

    fn a_minor() -> Scale {
        Scale {
            tonic: PitchClass::new(9),
            kind: ScaleKind::NaturalMinor,
        }
    }

    #[test]
    fn progressions_are_deterministic_and_structured() {
        let a = generate_chords(c_major(), 8, Seed(7));
        let b = generate_chords(c_major(), 8, Seed(7));
        let c = generate_chords(c_major(), 8, Seed(8));
        assert_eq!(a, b, "same seed, same progression");
        assert_ne!(a, c, "different seed should differ");
        assert_eq!(a.len(), 8);
        // Structure: starts on I, cadences V -> I.
        assert_eq!(a[0].symbol(), "C");
        assert_eq!(
            a[6].symbol(),
            "G",
            "second-to-last bar must be the dominant"
        );
        assert_eq!(a[7].symbol(), "C");
    }

    #[test]
    fn minor_keys_get_a_major_dominant() {
        let prog = generate_chords(a_minor(), 4, Seed(1));
        assert_eq!(
            prog[2].symbol(),
            "E",
            "harmonic-practice V in A minor is E major"
        );
        assert_eq!(prog[3].symbol(), "Am");
    }

    #[test]
    fn all_progression_chords_are_diatonic_triads() {
        let triads = diatonic_triads(c_major());
        let prog = generate_chords(c_major(), 16, Seed(42));
        for chord in &prog {
            assert!(
                triads.contains(chord),
                "{} is not diatonic in C major",
                chord.symbol()
            );
        }
    }

    #[test]
    fn melody_targets_chord_tones_on_strong_beats() {
        let prog = generate_chords(c_major(), 8, Seed(3));
        let melody = generate_melody(&prog, c_major(), Seed(3));
        for note in melody.notes() {
            let bar = usize::try_from(note.start.0 / BAR).unwrap();
            let offset = note.start.0 % BAR;
            if offset == 0 {
                // Beat 1 must be a tone of that bar's chord.
                let chord = &prog[bar];
                let tones: Vec<u8> = chord
                    .quality
                    .intervals()
                    .iter()
                    .map(|&i| (chord.root.index() + i) % 12)
                    .collect();
                assert!(
                    tones.contains(&(note.pitch.note % 12)),
                    "bar {bar}: melody note {} not in {}",
                    note.pitch.note,
                    chord.symbol()
                );
            }
            assert!((60..=84).contains(&note.pitch.note), "melody range");
        }
    }

    #[test]
    fn bass_starts_every_bar_on_the_root() {
        let prog = generate_chords(a_minor(), 8, Seed(9));
        let bass = generate_bass(&prog, Seed(9));
        for (bar, chord) in prog.iter().enumerate() {
            let downbeat = bass
                .notes()
                .iter()
                .find(|n| n.start.0 == i64::try_from(bar).unwrap() * BAR)
                .expect("bass downbeat every bar");
            assert_eq!(
                downbeat.pitch.note % 12,
                chord.root.index(),
                "bar {bar} root"
            );
        }
    }

    #[test]
    fn drums_are_deterministic_and_land_on_the_grid() {
        for style in [DrumStyle::Basic, DrumStyle::FourOnFloor, DrumStyle::LoFi] {
            let a = generate_drums(4, style, Seed(5));
            let b = generate_drums(4, style, Seed(5));
            assert_eq!(a, b);
            assert!(!a.notes().is_empty());
            // Snare backbeats on 2 and 4 in every style.
            assert!(a
                .notes()
                .iter()
                .any(|n| n.pitch.note == SNARE && n.start.0 == PPQ));
        }
    }

    #[test]
    fn chord_pattern_covers_the_whole_progression() {
        let prog = generate_chords(c_major(), 4, Seed(2));
        let pattern = chords_to_pattern(&prog, 3, 80);
        assert_eq!(pattern.length(), Tick(4 * BAR));
        assert_eq!(pattern.notes().len(), 12); // 4 triads
    }
}
