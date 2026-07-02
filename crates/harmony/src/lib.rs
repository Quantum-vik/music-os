//! Music theory engine: pitch classes, scales, chords, progressions, voice leading.
//!
//! Phase 1 milestone 1 covers pitch classes, scales, and chord spelling — the
//! vocabulary the composers and the reviewer agent build on (`docs/05` §2).
//! Progression grammars and voice-leading validation land in milestone 2.

use musicos_core_types::Pitch;
use serde::{Deserialize, Serialize};

/// One of the twelve pitch classes, as a chromatic index (0 = C … 11 = B).
///
/// Enharmonic *spelling* (C♯ vs D♭) is a separate concern layered on for
/// MusicXML round-trips later; MIDI-facing code only needs the chromatic index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PitchClass(u8);

impl PitchClass {
    /// Creates a pitch class from a chromatic index, taken modulo 12.
    pub fn new(index: u8) -> PitchClass {
        PitchClass(index % 12)
    }

    /// The chromatic index (0–11).
    pub fn index(self) -> u8 {
        self.0
    }

    /// Pitch class of a MIDI note number.
    pub fn of(pitch: Pitch) -> PitchClass {
        PitchClass(pitch.note % 12)
    }

    /// Transposes by `semitones` (wraps around the octave).
    pub fn transposed(self, semitones: i8) -> PitchClass {
        let idx = (i16::from(self.0) + i16::from(semitones)).rem_euclid(12);
        PitchClass(u8::try_from(idx).expect("rem_euclid(12) is 0..=11"))
    }

    /// The pitch in a given octave (octave 4 contains middle C = 60), clamped
    /// to the MIDI range.
    pub fn in_octave(self, octave: i8) -> Pitch {
        let note = (i16::from(octave) + 1) * 12 + i16::from(self.0);
        Pitch::new(u8::try_from(note.clamp(0, 127)).expect("clamped to 0..=127"))
    }
}

/// Scale species, described by their interval pattern from the tonic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ScaleKind {
    /// Ionian mode (natural major).
    Major,
    /// Aeolian mode (natural minor).
    NaturalMinor,
    /// Harmonic minor.
    HarmonicMinor,
    /// Melodic minor (ascending form).
    MelodicMinor,
    /// Dorian mode.
    Dorian,
    /// Mixolydian mode.
    Mixolydian,
    /// Major pentatonic.
    MajorPentatonic,
    /// Minor pentatonic.
    MinorPentatonic,
}

impl ScaleKind {
    /// Semitone offsets from the tonic, ascending within one octave.
    pub fn intervals(self) -> &'static [u8] {
        match self {
            ScaleKind::Major => &[0, 2, 4, 5, 7, 9, 11],
            ScaleKind::NaturalMinor => &[0, 2, 3, 5, 7, 8, 10],
            ScaleKind::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            ScaleKind::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            ScaleKind::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            ScaleKind::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            ScaleKind::MajorPentatonic => &[0, 2, 4, 7, 9],
            ScaleKind::MinorPentatonic => &[0, 3, 5, 7, 10],
        }
    }
}

/// A scale: a tonic plus a [`ScaleKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scale {
    /// The tonic pitch class.
    pub tonic: PitchClass,
    /// The scale species.
    pub kind: ScaleKind,
}

impl Scale {
    /// Whether the scale contains a pitch class.
    pub fn contains(&self, pc: PitchClass) -> bool {
        let rel = (i16::from(pc.index()) - i16::from(self.tonic.index())).rem_euclid(12);
        let rel = u8::try_from(rel).expect("rem_euclid(12) is 0..=11");
        self.kind.intervals().contains(&rel)
    }

    /// The scale degrees as pitch classes, starting at the tonic.
    pub fn pitch_classes(&self) -> Vec<PitchClass> {
        self.kind
            .intervals()
            .iter()
            .map(|&i| {
                self.tonic
                    .transposed(i8::try_from(i).expect("intervals < 12"))
            })
            .collect()
    }
}

/// Chord qualities, described by their interval structure over the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ChordQuality {
    /// Major triad.
    Major,
    /// Minor triad.
    Minor,
    /// Diminished triad.
    Diminished,
    /// Augmented triad.
    Augmented,
    /// Dominant seventh.
    Dominant7,
    /// Major seventh.
    Major7,
    /// Minor seventh.
    Minor7,
    /// Suspended fourth.
    Sus4,
}

impl ChordQuality {
    /// Semitone offsets from the root.
    pub fn intervals(self) -> &'static [u8] {
        match self {
            ChordQuality::Major => &[0, 4, 7],
            ChordQuality::Minor => &[0, 3, 7],
            ChordQuality::Diminished => &[0, 3, 6],
            ChordQuality::Augmented => &[0, 4, 8],
            ChordQuality::Dominant7 => &[0, 4, 7, 10],
            ChordQuality::Major7 => &[0, 4, 7, 11],
            ChordQuality::Minor7 => &[0, 3, 7, 10],
            ChordQuality::Sus4 => &[0, 5, 7],
        }
    }
}

/// A chord symbol: root plus quality. Extensions/inversions land in milestone 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Chord {
    /// The chord root.
    pub root: PitchClass,
    /// The chord quality.
    pub quality: ChordQuality,
}

impl Chord {
    /// Spells the chord as concrete pitches in close position from `octave`.
    pub fn pitches(&self, octave: i8) -> Vec<Pitch> {
        let root = self.root.in_octave(octave);
        self.quality
            .intervals()
            .iter()
            .map(|&i| Pitch::new((root.note + i).min(127)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_major_scale_membership() {
        let c_major = Scale {
            tonic: PitchClass::new(0),
            kind: ScaleKind::Major,
        };
        assert!(c_major.contains(PitchClass::new(4))); // E
        assert!(!c_major.contains(PitchClass::new(6))); // F#
        assert_eq!(c_major.pitch_classes().len(), 7);
    }

    #[test]
    fn scale_membership_is_transposition_invariant() {
        for tonic in 0..12u8 {
            let scale = Scale {
                tonic: PitchClass::new(tonic),
                kind: ScaleKind::Dorian,
            };
            // The tonic and the fifth are in every dorian scale.
            assert!(scale.contains(PitchClass::new(tonic)));
            assert!(scale.contains(PitchClass::new(tonic).transposed(7)));
        }
    }

    #[test]
    fn chord_spelling_from_octave() {
        // A minor triad in octave 3: A2? octave 3 root = 57 (A3).
        let am = Chord {
            root: PitchClass::new(9),
            quality: ChordQuality::Minor,
        };
        let notes: Vec<u8> = am.pitches(3).iter().map(|p| p.note).collect();
        assert_eq!(notes, vec![57, 60, 64]); // A3, C4, E4
    }

    #[test]
    fn middle_c_octave_convention() {
        assert_eq!(PitchClass::new(0).in_octave(4).note, 60);
    }
}
