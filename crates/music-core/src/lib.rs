//! Symbolic music model: notes, patterns, and pattern transformations.
//!
//! A [`Pattern`] is the workhorse of the Music context (`docs/05` §1): an
//! ordered collection of [`Note`]s with a declared length. All transformations
//! are pure `Pattern -> Pattern` functions; anything stochastic takes an
//! explicit [`Seed`] so results replay bit-identically (NFR-4).

use musicos_core_types::{Pitch, Seed, Tick, Velocity};
use serde::{Deserialize, Serialize};

/// A single note event within a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Note {
    /// Pitch of the note.
    pub pitch: Pitch,
    /// Note-on velocity.
    pub velocity: Velocity,
    /// Start position relative to the pattern origin. Never negative.
    pub start: Tick,
    /// Duration in ticks. Always positive.
    pub duration: Tick,
}

impl Note {
    /// End position (`start + duration`).
    pub fn end(&self) -> Tick {
        self.start + self.duration
    }
}

/// An immutable, tick-sorted collection of notes with a declared length.
///
/// Invariants (enforced by every constructor and transformation):
/// - notes are sorted by `(start, pitch.note)`;
/// - all starts are `>= 0` and all durations `> 0`;
/// - `length >=` the end of the last note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pattern {
    notes: Vec<Note>,
    length: Tick,
}

impl Pattern {
    /// Builds a pattern from notes, sorting them and growing `length` to fit.
    ///
    /// # Errors
    /// Returns [`PatternError`] if any note starts before zero or has a
    /// non-positive duration.
    pub fn new(mut notes: Vec<Note>, length: Tick) -> Result<Pattern, PatternError> {
        for n in &notes {
            if n.start < Tick::ZERO {
                return Err(PatternError::NegativeStart(n.start));
            }
            if n.duration <= Tick::ZERO {
                return Err(PatternError::NonPositiveDuration(n.duration));
            }
        }
        notes.sort_by_key(|n| (n.start, n.pitch.note));
        let content_end = notes.iter().map(Note::end).max().unwrap_or(Tick::ZERO);
        Ok(Pattern {
            notes,
            length: length.max(content_end),
        })
    }

    /// The empty pattern of a given length.
    pub fn empty(length: Tick) -> Pattern {
        Pattern {
            notes: Vec::new(),
            length,
        }
    }

    /// The notes, sorted by `(start, pitch)`.
    pub fn notes(&self) -> &[Note] {
        &self.notes
    }

    /// Declared pattern length.
    pub fn length(&self) -> Tick {
        self.length
    }

    /// Chromatic transposition by `semitones`, clamped to the MIDI range.
    pub fn transposed(&self, semitones: i8) -> Pattern {
        self.map_notes(|n| Note {
            pitch: n.pitch.transposed(semitones),
            ..n
        })
    }

    /// Quantizes note starts to a grid.
    ///
    /// `strength_pct` (0–100) moves each note that fraction of the way to the
    /// nearest grid line — partial quantization preserves feel (`docs/05` §3).
    /// Durations are unchanged; starts never leave `[0, length]`.
    ///
    /// # Panics
    /// Panics if `grid` is not positive.
    pub fn quantized(&self, grid: Tick, strength_pct: u8) -> Pattern {
        assert!(grid > Tick::ZERO, "grid must be positive");
        let strength = i64::from(strength_pct.min(100));
        self.map_notes(|n| {
            let target = nearest_multiple(n.start.0, grid.0);
            let moved = n.start.0 + (target - n.start.0) * strength / 100;
            Note {
                start: Tick(moved.clamp(0, self.length.0)),
                ..n
            }
        })
    }

    /// Humanizes timing and velocity with deterministic, seeded jitter.
    ///
    /// Each note's start moves by up to `±timing` ticks and its velocity by up
    /// to `±velocity` steps. The same `(pattern, seed, params)` always produces
    /// the same output (NFR-4).
    pub fn humanized(&self, seed: Seed, timing: Tick, velocity: u8) -> Pattern {
        let mut rng = rng::SplitMix64::new(seed);
        self.map_notes(|n| {
            let dt = rng.in_range(timing.0);
            let dv = i32::try_from(rng.in_range(i64::from(velocity))).expect("|dv| <= 127");
            let vel = Velocity::clamped(i32::from(n.velocity.get()) + dv);
            Note {
                start: Tick((n.start.0 + dt).clamp(0, self.length.0)),
                velocity: vel,
                ..n
            }
        })
    }

    fn map_notes(&self, f: impl FnMut(Note) -> Note) -> Pattern {
        let mut notes: Vec<Note> = self.notes.iter().copied().map(f).collect();
        notes.sort_by_key(|n| (n.start, n.pitch.note));
        let content_end = notes.iter().map(Note::end).max().unwrap_or(Tick::ZERO);
        Pattern {
            notes,
            length: self.length.max(content_end),
        }
    }
}

fn nearest_multiple(value: i64, grid: i64) -> i64 {
    let down = value.div_euclid(grid) * grid;
    let up = down + grid;
    if value - down <= up - value {
        down
    } else {
        up
    }
}

/// Errors from pattern construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    /// A note started before tick zero.
    #[error("note starts before tick zero (at {0:?})")]
    NegativeStart(Tick),
    /// A note had a zero or negative duration.
    #[error("note has non-positive duration ({0:?})")]
    NonPositiveDuration(Tick),
}

pub mod rng {
    //! Deterministic, seeded randomness for the domain (NFR-4).
    //!
    //! Every stochastic domain operation takes an explicit [`Seed`] and
    //! flows through this RNG — never
    //! ambient/global RNG state. SplitMix64: tiny, fast, and identical on
    //! every platform.

    use musicos_core_types::Seed;

    /// A deterministic SplitMix64 generator.
    #[derive(Debug, Clone)]
    pub struct SplitMix64(u64);

    impl SplitMix64 {
        /// Creates a generator from a seed.
        pub fn new(seed: Seed) -> Self {
            SplitMix64(seed.0)
        }

        /// Next raw 64-bit value.
        pub fn next_u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// Uniform value in `[-max, max]`; `0` when `max <= 0`.
        pub fn in_range(&mut self, max: i64) -> i64 {
            if max <= 0 {
                return 0;
            }
            let span = u64::try_from(2 * max + 1).expect("max is positive");
            let r = i64::try_from(self.next_u64() % span).expect("span fits i64");
            r - max
        }

        /// Uniform index in `[0, len)`; `0` when `len == 0`.
        pub fn index(&mut self, len: usize) -> usize {
            if len == 0 {
                return 0;
            }
            usize::try_from(self.next_u64() % len as u64).expect("index fits usize")
        }

        /// True with probability `pct`/100.
        pub fn chance(&mut self, pct: u8) -> bool {
            (self.next_u64() % 100) < u64::from(pct.min(100))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicos_core_types::PPQ;
    use proptest::prelude::*;

    fn note(start: i64, dur: i64, pitch: u8) -> Note {
        Note {
            pitch: Pitch::new(pitch),
            velocity: Velocity::MF,
            start: Tick(start),
            duration: Tick(dur),
        }
    }

    #[test]
    fn construction_sorts_and_grows_length() {
        let p = Pattern::new(vec![note(960, 480, 64), note(0, 480, 60)], Tick::ZERO).unwrap();
        assert_eq!(p.notes()[0].start, Tick(0));
        assert_eq!(p.length(), Tick(1440));
    }

    #[test]
    fn construction_rejects_invalid_notes() {
        assert!(Pattern::new(vec![note(-1, 10, 60)], Tick::ZERO).is_err());
        assert!(Pattern::new(vec![note(0, 0, 60)], Tick::ZERO).is_err());
    }

    #[test]
    fn full_quantize_snaps_to_grid() {
        let p = Pattern::new(vec![note(1000, 100, 60), note(430, 100, 62)], Tick(PPQ * 4)).unwrap();
        let q = p.quantized(Tick(PPQ / 2), 100);
        assert_eq!(q.notes()[0].start, Tick(480));
        assert_eq!(q.notes()[1].start, Tick(960));
    }

    #[test]
    fn humanize_is_deterministic_per_seed() {
        let p = Pattern::new(vec![note(0, 480, 60), note(960, 480, 64)], Tick(PPQ * 2)).unwrap();
        let a = p.humanized(Seed(42), Tick(30), 10);
        let b = p.humanized(Seed(42), Tick(30), 10);
        let c = p.humanized(Seed(43), Tick(30), 10);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    proptest! {
        #[test]
        fn transpose_up_then_down_is_identity_away_from_clamp(
            starts in proptest::collection::vec(0i64..10_000, 1..40),
            semis in 1i8..=12,
        ) {
            let notes: Vec<Note> = starts.iter()
                .map(|&s| note(s, 240, 60)) // note 60 ± 12 never clamps
                .collect();
            let p = Pattern::new(notes, Tick(20_000)).unwrap();
            prop_assert_eq!(p.transposed(semis).transposed(-semis), p);
        }

        #[test]
        fn quantize_at_full_strength_is_idempotent(
            starts in proptest::collection::vec(0i64..8_000, 1..40),
        ) {
            let notes: Vec<Note> = starts.iter().map(|&s| note(s, 120, 60)).collect();
            let p = Pattern::new(notes, Tick(10_000)).unwrap();
            let q = p.quantized(Tick(240), 100);
            prop_assert_eq!(q.quantized(Tick(240), 100), q.clone());
        }
    }
}
