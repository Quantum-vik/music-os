//! Musical timeline: tempo maps, time signatures, and tick/sample conversion.
//!
//! Wall-clock and sample time are always *derived* from integer ticks through
//! the [`TempoMap`] (ADR-0004): conversion uses exact integer arithmetic
//! (`i128` intermediates), so the same map and tick produce the same
//! microsecond position on every platform.

use musicos_core_types::{Tempo, Tick, TimeSignature, PPQ};
use serde::{Deserialize, Serialize};

/// Tempo changes over the timeline.
///
/// Invariant TM1 (`docs/03` §3): entries are sorted by tick, ticks are unique,
/// and an entry at tick 0 always exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TempoMap {
    entries: Vec<(Tick, Tempo)>,
}

impl TempoMap {
    /// A constant-tempo map.
    pub fn constant(tempo: Tempo) -> TempoMap {
        TempoMap {
            entries: vec![(Tick::ZERO, tempo)],
        }
    }

    /// Builds a map from entries, sorting them.
    ///
    /// # Errors
    /// Returns [`TimelineError`] if entries are empty, contain duplicate
    /// ticks, a negative tick, or no entry at tick 0.
    pub fn new(mut entries: Vec<(Tick, Tempo)>) -> Result<TempoMap, TimelineError> {
        if entries.is_empty() {
            return Err(TimelineError::Empty);
        }
        entries.sort_by_key(|(t, _)| *t);
        if entries[0].0 != Tick::ZERO {
            return Err(TimelineError::NoOrigin);
        }
        if entries.first().is_some_and(|(t, _)| *t < Tick::ZERO) {
            return Err(TimelineError::NegativeTick);
        }
        if entries.windows(2).any(|w| w[0].0 == w[1].0) {
            return Err(TimelineError::DuplicateTick);
        }
        Ok(TempoMap { entries })
    }

    /// The tempo change entries, sorted by tick.
    pub fn entries(&self) -> &[(Tick, Tempo)] {
        &self.entries
    }

    /// Sets (adds or replaces) the tempo entry at exactly `at`, returning the
    /// entry it replaced, if any.
    ///
    /// # Errors
    /// Returns [`TimelineError::NegativeTick`] if `at` is negative.
    pub fn set(&mut self, at: Tick, tempo: Tempo) -> Result<Option<Tempo>, TimelineError> {
        if at < Tick::ZERO {
            return Err(TimelineError::NegativeTick);
        }
        match self.entries.binary_search_by_key(&at, |(t, _)| *t) {
            Ok(i) => {
                let prev = self.entries[i].1;
                self.entries[i].1 = tempo;
                Ok(Some(prev))
            }
            Err(i) => {
                self.entries.insert(i, (at, tempo));
                Ok(None)
            }
        }
    }

    /// Removes the tempo entry at exactly `at`, returning it.
    ///
    /// # Errors
    /// Returns [`TimelineError::NoOrigin`] when asked to remove the origin
    /// entry (an entry at tick 0 must always exist — invariant TM1).
    pub fn remove(&mut self, at: Tick) -> Result<Option<Tempo>, TimelineError> {
        if at == Tick::ZERO {
            return Err(TimelineError::NoOrigin);
        }
        match self.entries.binary_search_by_key(&at, |(t, _)| *t) {
            Ok(i) => Ok(Some(self.entries.remove(i).1)),
            Err(_) => Ok(None),
        }
    }

    /// Tempo in effect at `at`.
    pub fn tempo_at(&self, at: Tick) -> Tempo {
        match self.entries.binary_search_by_key(&at, |(t, _)| *t) {
            Ok(i) => self.entries[i].1,
            Err(0) => self.entries[0].1, // before origin: clamp
            Err(i) => self.entries[i - 1].1,
        }
    }

    /// Converts a tick position to microseconds from the timeline origin.
    ///
    /// Exact integer arithmetic; segments accumulate as
    /// `Δticks × μs-per-quarter / PPQ` with `i128` intermediates.
    pub fn tick_to_micros(&self, at: Tick) -> i64 {
        let at = at.max(Tick::ZERO);
        let mut micros: i128 = 0;
        for (i, &(start, tempo)) in self.entries.iter().enumerate() {
            if start >= at {
                break;
            }
            let seg_end = self
                .entries
                .get(i + 1)
                .map_or(at, |&(next, _)| next.min(at));
            let dticks = i128::from((seg_end - start).0);
            micros += dticks * i128::from(tempo.micros_per_quarter) / i128::from(PPQ);
        }
        i64::try_from(micros).expect("timeline duration fits i64 microseconds")
    }

    /// Converts a tick position to a sample frame index at `sample_rate` Hz.
    pub fn tick_to_samples(&self, at: Tick, sample_rate: u32) -> i64 {
        let micros = i128::from(self.tick_to_micros(at));
        i64::try_from(micros * i128::from(sample_rate) / 1_000_000)
            .expect("sample position fits i64")
    }
}

impl Default for TempoMap {
    fn default() -> Self {
        TempoMap::constant(Tempo::DEFAULT)
    }
}

/// Time-signature changes over the timeline. Same invariants as [`TempoMap`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureMap {
    entries: Vec<(Tick, TimeSignature)>,
}

impl SignatureMap {
    /// A constant-meter map.
    pub fn constant(sig: TimeSignature) -> SignatureMap {
        SignatureMap {
            entries: vec![(Tick::ZERO, sig)],
        }
    }

    /// The signature change entries, sorted by tick.
    pub fn entries(&self) -> &[(Tick, TimeSignature)] {
        &self.entries
    }

    /// Signature in effect at `at`.
    pub fn signature_at(&self, at: Tick) -> TimeSignature {
        match self.entries.binary_search_by_key(&at, |(t, _)| *t) {
            Ok(i) => self.entries[i].1,
            Err(0) => self.entries[0].1,
            Err(i) => self.entries[i - 1].1,
        }
    }
}

impl Default for SignatureMap {
    fn default() -> Self {
        SignatureMap::constant(TimeSignature::COMMON)
    }
}

/// Errors from timeline map construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TimelineError {
    /// The map had no entries.
    #[error("map must have at least one entry")]
    Empty,
    /// No entry at tick zero.
    #[error("map must have an entry at tick 0")]
    NoOrigin,
    /// An entry had a negative tick.
    #[error("map entries must not be at negative ticks")]
    NegativeTick,
    /// Two entries shared the same tick.
    #[error("map entries must have unique ticks")]
    DuplicateTick,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_tempo_conversion_is_exact() {
        // 120 BPM: one quarter = 500_000 µs.
        let map = TempoMap::constant(Tempo::DEFAULT);
        assert_eq!(map.tick_to_micros(Tick(PPQ)), 500_000);
        assert_eq!(map.tick_to_micros(Tick(PPQ * 4)), 2_000_000);
        // 48 kHz: one quarter at 120 BPM = 24_000 samples.
        assert_eq!(map.tick_to_samples(Tick(PPQ), 48_000), 24_000);
    }

    #[test]
    fn tempo_changes_accumulate_per_segment() {
        // 120 BPM for 1 quarter, then 60 BPM.
        let map = TempoMap::new(vec![
            (Tick::ZERO, Tempo::DEFAULT),
            (
                Tick(PPQ),
                Tempo {
                    micros_per_quarter: 1_000_000,
                },
            ),
        ])
        .unwrap();
        assert_eq!(map.tick_to_micros(Tick(PPQ)), 500_000);
        assert_eq!(map.tick_to_micros(Tick(PPQ * 2)), 1_500_000);
        assert_eq!(map.tempo_at(Tick(PPQ - 1)), Tempo::DEFAULT);
        assert_eq!(map.tempo_at(Tick(PPQ)).micros_per_quarter, 1_000_000);
    }

    #[test]
    fn invariants_are_enforced() {
        assert_eq!(TempoMap::new(vec![]).unwrap_err(), TimelineError::Empty);
        assert_eq!(
            TempoMap::new(vec![(Tick(10), Tempo::DEFAULT)]).unwrap_err(),
            TimelineError::NoOrigin
        );
        assert_eq!(
            TempoMap::new(vec![
                (Tick::ZERO, Tempo::DEFAULT),
                (Tick::ZERO, Tempo::DEFAULT)
            ])
            .unwrap_err(),
            TimelineError::DuplicateTick
        );
    }

    #[test]
    fn set_and_remove_preserve_invariants() {
        let mut map = TempoMap::default();
        let slow = Tempo {
            micros_per_quarter: 1_000_000,
        };
        assert_eq!(map.set(Tick(PPQ), slow).unwrap(), None);
        assert_eq!(map.set(Tick(PPQ), Tempo::DEFAULT).unwrap(), Some(slow));
        assert_eq!(map.remove(Tick(PPQ)).unwrap(), Some(Tempo::DEFAULT));
        assert_eq!(map.remove(Tick(PPQ)).unwrap(), None);
        assert_eq!(map.remove(Tick::ZERO).unwrap_err(), TimelineError::NoOrigin);
        assert_eq!(
            map.set(Tick(-1), slow).unwrap_err(),
            TimelineError::NegativeTick
        );
        assert_eq!(map.entries().len(), 1); // origin intact
    }
}
