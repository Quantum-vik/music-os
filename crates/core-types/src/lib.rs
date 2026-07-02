//! Core identifiers, time types, and error types shared by every MusicOS crate.
//!
//! This is the leaf crate of the workspace: it depends on nothing internal and
//! (almost) nothing external, and every other crate may depend on it. See
//! `docs/03_Domain_Model.md` §2 for the full value-object catalogue; types land
//! here incrementally through Phase 1.

/// Musical time in ticks at a fixed resolution of [`PPQ`] pulses per quarter note.
///
/// Musical time is always integer ticks — never floating point — so that edits,
/// replays, and cross-platform runs are bit-identical (ADR-0004). Wall-clock
/// time is derived through the tempo map in `musicos-timeline`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Tick(pub i64);

/// Pulses per quarter note used by [`Tick`]. Divisible by 2–8, 12, 16, 32, 64.
pub const PPQ: i64 = 960;

impl Tick {
    /// The zero tick (start of the timeline).
    pub const ZERO: Tick = Tick(0);

    /// Number of whole quarter notes this tick position represents, rounding down.
    pub fn quarters(self) -> i64 {
        self.0.div_euclid(PPQ)
    }
}

/// Opaque identifier for a project.
///
/// Phase 0 placeholder: becomes a UUID-backed newtype at the persistence
/// boundary in Phase 1 (`docs/03_Domain_Model.md` §3, invariant ID1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectId(pub u64);

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
}
