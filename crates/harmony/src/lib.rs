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

/// Parses a note name (`C`, `F#`, `Bb`, `c#`) into a pitch class.
///
/// # Errors
/// Returns [`ParseError`] on anything else.
pub fn parse_note_name(name: &str) -> Result<PitchClass, ParseError> {
    let mut chars = name.trim().chars();
    let letter = chars.next().ok_or_else(|| ParseError(name.to_string()))?;
    let base: i16 = match letter.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return Err(ParseError(name.to_string())),
    };
    let accidental: i16 = match chars.as_str() {
        "" => 0,
        "#" => 1,
        "b" => -1,
        "##" => 2,
        "bb" => -2,
        _ => return Err(ParseError(name.to_string())),
    };
    Ok(PitchClass::new(
        u8::try_from((base + accidental).rem_euclid(12)).expect("mod 12"),
    ))
}

/// Parses a scale kind name (`major`, `minor`, `dorian`, `harmonic_minor`, …).
///
/// # Errors
/// Returns [`ParseError`] for unknown names.
pub fn parse_scale_kind(name: &str) -> Result<ScaleKind, ParseError> {
    match name.trim().to_ascii_lowercase().as_str() {
        "major" | "ionian" => Ok(ScaleKind::Major),
        "minor" | "natural_minor" | "aeolian" => Ok(ScaleKind::NaturalMinor),
        "harmonic_minor" => Ok(ScaleKind::HarmonicMinor),
        "melodic_minor" => Ok(ScaleKind::MelodicMinor),
        "dorian" => Ok(ScaleKind::Dorian),
        "mixolydian" => Ok(ScaleKind::Mixolydian),
        "major_pentatonic" => Ok(ScaleKind::MajorPentatonic),
        "minor_pentatonic" => Ok(ScaleKind::MinorPentatonic),
        _ => Err(ParseError(name.to_string())),
    }
}

impl Chord {
    /// Parses a chord symbol: `C`, `Am`, `F#m`, `Bdim`, `Gaug`, `D7`,
    /// `Cmaj7`, `Em7`, `Asus4` (root note name + quality suffix).
    ///
    /// # Errors
    /// Returns [`ParseError`] for unknown symbols.
    pub fn parse(symbol: &str) -> Result<Chord, ParseError> {
        let s = symbol.trim();
        // Longest valid note-name prefix: letter plus up to two accidentals.
        let mut split = 1;
        for (i, c) in s.char_indices().skip(1) {
            if c == '#' || c == 'b' {
                split = i + 1;
            } else {
                break;
            }
        }
        if s.is_empty() {
            return Err(ParseError(symbol.to_string()));
        }
        let root = parse_note_name(&s[..split])?;
        let quality = match &s[split..] {
            "" | "maj" | "M" => ChordQuality::Major,
            "m" | "min" | "-" => ChordQuality::Minor,
            "dim" | "o" => ChordQuality::Diminished,
            "aug" | "+" => ChordQuality::Augmented,
            "7" => ChordQuality::Dominant7,
            "maj7" | "M7" => ChordQuality::Major7,
            "m7" | "min7" => ChordQuality::Minor7,
            "sus4" | "sus" => ChordQuality::Sus4,
            _ => return Err(ParseError(symbol.to_string())),
        };
        Ok(Chord { root, quality })
    }

    /// The canonical symbol for this chord (`Am`, `F#dim`, `G7`, …).
    pub fn symbol(&self) -> String {
        const NAMES: [&str; 12] = [
            "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
        ];
        let root = NAMES[usize::from(self.root.index())];
        let suffix = match self.quality {
            ChordQuality::Major => "",
            ChordQuality::Minor => "m",
            ChordQuality::Diminished => "dim",
            ChordQuality::Augmented => "aug",
            ChordQuality::Dominant7 => "7",
            ChordQuality::Major7 => "maj7",
            ChordQuality::Minor7 => "m7",
            ChordQuality::Sus4 => "sus4",
        };
        format!("{root}{suffix}")
    }
}

/// A symbol that could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("cannot parse '{0}'")]
pub struct ParseError(pub String);

#[cfg(test)]
mod parse_tests {
    use super::*;

    #[test]
    fn note_names_parse_with_accidentals() {
        assert_eq!(parse_note_name("C").unwrap().index(), 0);
        assert_eq!(parse_note_name("F#").unwrap().index(), 6);
        assert_eq!(parse_note_name("Bb").unwrap().index(), 10);
        assert_eq!(parse_note_name("Cb").unwrap().index(), 11);
        assert!(parse_note_name("H").is_err());
    }

    #[test]
    fn chord_symbols_round_trip() {
        for sym in [
            "C", "Am", "F#m", "Bdim", "Gaug", "D7", "Cmaj7", "Em7", "Asus4",
        ] {
            let chord = Chord::parse(sym).unwrap();
            assert_eq!(chord.symbol(), *sym, "{sym}");
            assert_eq!(Chord::parse(&chord.symbol()).unwrap(), chord);
        }
        assert!(Chord::parse("Xyz").is_err());
        assert!(Chord::parse("").is_err());
    }

    #[test]
    fn scale_kind_aliases() {
        assert_eq!(parse_scale_kind("major").unwrap(), ScaleKind::Major);
        assert_eq!(parse_scale_kind("MINOR").unwrap(), ScaleKind::NaturalMinor);
        assert_eq!(parse_scale_kind("dorian").unwrap(), ScaleKind::Dorian);
        assert!(parse_scale_kind("phrygian-dominant-9").is_err());
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
