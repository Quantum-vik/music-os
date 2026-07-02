//! Core identifiers, time types, and error types shared by every MusicOS crate.
//!
//! This is the leaf crate of the workspace: it depends on nothing internal and
//! only on `serde` externally (ADR note in `docs/02` §2). The full value-object
//! catalogue is specified in `docs/03_Domain_Model.md` §2.

use serde::{Deserialize, Serialize};

/// Pulses per quarter note used by [`Tick`]. Divisible by 2–8, 12, 16, 32, 64.
pub const PPQ: i64 = 960;

/// Musical time in ticks at a fixed resolution of [`PPQ`] pulses per quarter note.
///
/// Musical time is always integer ticks — never floating point — so that edits,
/// replays, and cross-platform runs are bit-identical (ADR-0004). Wall-clock
/// time is derived through the tempo map in `musicos-timeline`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Tick(pub i64);

impl Tick {
    /// The zero tick (start of the timeline).
    pub const ZERO: Tick = Tick(0);

    /// One quarter note.
    pub const QUARTER: Tick = Tick(PPQ);

    /// Number of whole quarter notes this tick position represents, rounding down.
    pub fn quarters(self) -> i64 {
        self.0.div_euclid(PPQ)
    }

    /// Saturating addition.
    pub fn saturating_add(self, rhs: Tick) -> Tick {
        Tick(self.0.saturating_add(rhs.0))
    }
}

impl core::ops::Add for Tick {
    type Output = Tick;
    fn add(self, rhs: Tick) -> Tick {
        Tick(self.0 + rhs.0)
    }
}

impl core::ops::Sub for Tick {
    type Output = Tick;
    fn sub(self, rhs: Tick) -> Tick {
        Tick(self.0 - rhs.0)
    }
}

/// Pitch as a MIDI note number plus an optional microtonal offset in cents.
///
/// `cents` covers microtonality and humanization without a second pitch model
/// (`docs/03` §2); SMF export maps non-zero cents to pitch bend (Phase 1: cents
/// are preserved in the model but dropped at the SMF boundary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Pitch {
    /// MIDI note number (0–127; 60 = middle C).
    pub note: u8,
    /// Microtonal offset in cents (−100..=100 kept by convention).
    #[serde(default, skip_serializing_if = "is_zero_cents")]
    pub cents: i16,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if requires fn(&T)
fn is_zero_cents(c: &i16) -> bool {
    *c == 0
}

impl Pitch {
    /// A pitch on the 12-TET grid with no microtonal offset.
    pub fn new(note: u8) -> Pitch {
        Pitch { note, cents: 0 }
    }

    /// Transposes by `semitones`, clamping to the valid MIDI range 0–127.
    pub fn transposed(self, semitones: i8) -> Pitch {
        let note = i16::from(self.note) + i16::from(semitones);
        Pitch {
            note: u8::try_from(note.clamp(0, 127)).expect("clamped to 0..=127"),
            cents: self.cents,
        }
    }
}

/// MIDI velocity, validated to 1–127 for note-ons (0 means note-off on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Velocity(u8);

impl Velocity {
    /// A sensible default velocity (mezzo-forte).
    pub const MF: Velocity = Velocity(80);

    /// Creates a velocity, returning `None` outside 1..=127.
    pub fn new(v: u8) -> Option<Velocity> {
        (1..=127).contains(&v).then_some(Velocity(v))
    }

    /// Creates a velocity, clamping into 1..=127.
    pub fn clamped(v: i32) -> Velocity {
        Velocity(u8::try_from(v.clamp(1, 127)).expect("clamped to 1..=127"))
    }

    /// The raw MIDI value (1–127).
    pub fn get(self) -> u8 {
        self.0
    }
}

/// A time signature such as 4/4 or 7/8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TimeSignature {
    /// Beats per bar.
    pub numerator: u8,
    /// Note value of one beat (power of two: 1, 2, 4, 8, 16, 32).
    pub denominator: u8,
}

impl TimeSignature {
    /// Common time, 4/4.
    pub const COMMON: TimeSignature = TimeSignature {
        numerator: 4,
        denominator: 4,
    };

    /// Length of one bar in ticks.
    pub fn bar_ticks(self) -> Tick {
        Tick(i64::from(self.numerator) * PPQ * 4 / i64::from(self.denominator))
    }
}

/// Tempo stored exactly as microseconds per quarter note, as in SMF.
///
/// Integer storage keeps tempo math exact (ADR-0004); BPM is a derived,
/// display-only value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tempo {
    /// Microseconds per quarter note (`500_000` = 120 BPM).
    pub micros_per_quarter: u32,
}

impl Tempo {
    /// 120 BPM.
    pub const DEFAULT: Tempo = Tempo {
        micros_per_quarter: 500_000,
    };

    /// Creates a tempo from beats per minute (approximate; storage stays exact).
    pub fn from_bpm(bpm: f64) -> Tempo {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // range-checked below
        let mpq = (60_000_000.0 / bpm.clamp(1.0, 1000.0)).round() as u32;
        Tempo {
            micros_per_quarter: mpq,
        }
    }

    /// Beats per minute (display only).
    pub fn bpm(self) -> f64 {
        60_000_000.0 / f64::from(self.micros_per_quarter)
    }
}

/// Seed for all randomness in the domain. Everything stochastic takes an
/// explicit seed so runs replay bit-identically (NFR-4, `docs/05` §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seed(pub u64);

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        ///
        /// Typed newtype id (`docs/03` §3, invariant ID1). Backed by `u64` in
        /// Phase 1; becomes UUID-backed at the persistence boundary.
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
            Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);
    };
}

id_type!(
    /// Identifier for a project.
    ProjectId
);
id_type!(
    /// Identifier for a track within a project.
    TrackId
);
id_type!(
    /// Identifier for a clip within a project.
    ClipId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ppq_is_divisible_by_common_grids() {
        for grid in [2, 3, 4, 5, 6, 8, 12, 16, 32, 64] {
            assert_eq!(PPQ % grid, 0, "PPQ must be divisible by {grid}");
        }
    }

    #[test]
    fn quarters_rounds_toward_negative_infinity() {
        assert_eq!(Tick(PPQ * 3 + 1).quarters(), 3);
        assert_eq!(Tick(-1).quarters(), -1);
    }

    #[test]
    fn pitch_transposition_clamps_to_midi_range() {
        assert_eq!(Pitch::new(60).transposed(7).note, 67);
        assert_eq!(Pitch::new(126).transposed(5).note, 127);
        assert_eq!(Pitch::new(2).transposed(-5).note, 0);
    }

    #[test]
    fn velocity_is_validated() {
        assert!(Velocity::new(0).is_none());
        assert!(Velocity::new(128).is_none());
        assert_eq!(Velocity::clamped(999).get(), 127);
    }

    #[test]
    fn bar_ticks_for_common_meters() {
        assert_eq!(TimeSignature::COMMON.bar_ticks(), Tick(PPQ * 4));
        let seven_eight = TimeSignature {
            numerator: 7,
            denominator: 8,
        };
        assert_eq!(seven_eight.bar_ticks(), Tick(PPQ * 7 / 2));
    }

    #[test]
    fn tempo_round_trips_through_bpm_display() {
        let t = Tempo::from_bpm(120.0);
        assert_eq!(t.micros_per_quarter, 500_000);
        assert!((t.bpm() - 120.0).abs() < 1e-9);
    }
}
