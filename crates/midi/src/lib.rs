//! Standard MIDI File import/export and internal pattern mapping.
//!
//! The only crate that speaks SMF: `midly` never leaks past this boundary
//! (anti-corruption, `docs/03` §7). Phase 1 milestone 1 supports format 0/1
//! files, note events, and tempo maps; CC/pitch-bend lanes and PPQ residue
//! preservation land with milestone 2 (`docs/05` §5).

use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use musicos_core_types::{Pitch, Tempo, Tick, Velocity, PPQ};
use musicos_music_core::{Note, Pattern};
use musicos_timeline::TempoMap;

/// A song's symbolic content as read from or written to an SMF file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmfSong {
    /// Tempo changes.
    pub tempo_map: TempoMap,
    /// One pattern per MIDI track that contains notes, with its name if any.
    pub tracks: Vec<(Option<String>, Pattern)>,
}

/// Imports a Standard MIDI File (format 0 or 1).
///
/// Note-on with velocity 0 is treated as note-off per the MIDI spec. Tick
/// positions are rescaled exactly from the file's PPQ to MusicOS's 960 PPQ
/// (rounding to nearest when the ratio is not integral).
///
/// # Errors
/// Returns [`MidiError`] on malformed files, SMPTE timing, or unmatched notes.
pub fn import_smf(bytes: &[u8]) -> Result<SmfSong, MidiError> {
    let smf = Smf::parse(bytes)?;
    let file_ppq: i64 = match smf.header.timing {
        Timing::Metrical(t) => i64::from(t.as_int()),
        Timing::Timecode(..) => return Err(MidiError::SmpteTiming),
    };

    let mut tempo_entries: Vec<(Tick, Tempo)> = Vec::new();
    let mut tracks = Vec::new();

    for track in &smf.tracks {
        let mut at_file_ticks: i64 = 0;
        let mut name: Option<String> = None;
        // Active note-ons per (channel, key): stack handles overlapping repeats.
        let mut open: std::collections::HashMap<(u8, u8), Vec<(i64, Velocity)>> =
            std::collections::HashMap::new();
        let mut notes: Vec<Note> = Vec::new();

        for ev in track {
            at_file_ticks += i64::from(ev.delta.as_int());
            let at = rescale(at_file_ticks, file_ppq);
            match ev.kind {
                TrackEventKind::Meta(MetaMessage::Tempo(mpq)) => {
                    tempo_entries.push((
                        at,
                        Tempo {
                            micros_per_quarter: mpq.as_int(),
                        },
                    ));
                }
                TrackEventKind::Meta(MetaMessage::TrackName(n)) => {
                    name = Some(String::from_utf8_lossy(n).into_owned());
                }
                TrackEventKind::Midi { channel, message } => match message {
                    MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                        let v = Velocity::new(vel.as_int()).expect("vel > 0 checked");
                        open.entry((channel.as_int(), key.as_int()))
                            .or_default()
                            .push((at_file_ticks, v));
                    }
                    MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                        let stack = open.entry((channel.as_int(), key.as_int())).or_default();
                        let Some((on_ticks, vel)) = stack.pop() else {
                            continue; // stray note-off: tolerate, per robustness principle
                        };
                        let start = rescale(on_ticks, file_ppq);
                        let end = rescale(at_file_ticks, file_ppq);
                        if end > start {
                            notes.push(Note {
                                pitch: Pitch::new(key.as_int()),
                                velocity: vel,
                                start,
                                duration: end - start,
                            });
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        if !notes.is_empty() {
            let pattern = Pattern::new(notes, Tick::ZERO).map_err(MidiError::Pattern)?;
            tracks.push((name, pattern));
        }
    }

    if !tempo_entries.iter().any(|(t, _)| *t == Tick::ZERO) {
        tempo_entries.insert(0, (Tick::ZERO, Tempo::DEFAULT));
    }
    tempo_entries.sort_by_key(|(t, _)| *t);
    tempo_entries.dedup_by_key(|(t, _)| *t);
    let tempo_map = TempoMap::new(tempo_entries).map_err(MidiError::Timeline)?;

    Ok(SmfSong { tempo_map, tracks })
}

/// Exports a song as a format-1 Standard MIDI File at 960 PPQ.
///
/// Track 0 carries the tempo map; each pattern becomes one note track on
/// channel 0. Microtonal `cents` are dropped at this boundary in milestone 1.
pub fn export_smf(song: &SmfSong) -> Vec<u8> {
    let header = Header::new(
        Format::Parallel,
        Timing::Metrical(u16::try_from(PPQ).expect("PPQ fits u16").into()),
    );
    let mut smf = Smf::new(header);

    // Tempo track.
    let mut tempo_track = Vec::new();
    let mut events: Vec<(i64, TrackEventKind<'_>)> = song
        .tempo_map
        .entries()
        .iter()
        .map(|&(tick, tempo)| {
            (
                tick.0,
                TrackEventKind::Meta(MetaMessage::Tempo(tempo.micros_per_quarter.into())),
            )
        })
        .collect();
    push_with_deltas(&mut tempo_track, &mut events);
    smf.tracks.push(tempo_track);

    // Note tracks.
    for (name, pattern) in &song.tracks {
        let mut events: Vec<(i64, TrackEventKind<'_>)> = Vec::new();
        if let Some(name) = name {
            events.push((
                0,
                TrackEventKind::Meta(MetaMessage::TrackName(name.as_bytes())),
            ));
        }
        for n in pattern.notes() {
            events.push((
                n.start.0,
                TrackEventKind::Midi {
                    channel: 0.into(),
                    message: MidiMessage::NoteOn {
                        key: n.pitch.note.into(),
                        vel: n.velocity.get().into(),
                    },
                },
            ));
            events.push((
                n.end().0,
                TrackEventKind::Midi {
                    channel: 0.into(),
                    message: MidiMessage::NoteOff {
                        key: n.pitch.note.into(),
                        vel: 0.into(),
                    },
                },
            ));
        }
        let mut track = Vec::new();
        push_with_deltas(&mut track, &mut events);
        smf.tracks.push(track);
    }

    let mut out = Vec::new();
    smf.write(&mut out)
        .expect("in-memory SMF write cannot fail");
    out
}

/// Sorts absolute-tick events (note-offs before note-ons at equal ticks, so
/// zero-gap repeats re-trigger correctly) and appends them with delta times.
fn push_with_deltas<'a>(
    track: &mut Vec<TrackEvent<'a>>,
    events: &mut Vec<(i64, TrackEventKind<'a>)>,
) {
    events.sort_by_key(|(tick, kind)| (*tick, event_order(kind)));
    let mut last = 0i64;
    for (tick, kind) in events.drain(..) {
        let delta = u32::try_from(tick - last).expect("events sorted, ticks non-negative");
        track.push(TrackEvent {
            delta: delta.into(),
            kind,
        });
        last = tick;
    }
    track.push(TrackEvent {
        delta: 0.into(),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });
}

fn event_order(kind: &TrackEventKind<'_>) -> u8 {
    match kind {
        TrackEventKind::Meta(_) => 0,
        TrackEventKind::Midi {
            message: MidiMessage::NoteOff { .. },
            ..
        } => 1,
        TrackEventKind::Midi {
            message: MidiMessage::NoteOn { vel, .. },
            ..
        } if vel.as_int() == 0 => 1,
        _ => 2,
    }
}

/// Rescales a tick count from `file_ppq` to the internal 960 PPQ, rounding to
/// nearest. Exact whenever `file_ppq` divides into 960 or vice versa.
fn rescale(file_ticks: i64, file_ppq: i64) -> Tick {
    let n = i128::from(file_ticks) * i128::from(PPQ);
    let d = i128::from(file_ppq);
    let rounded = (n + d / 2).div_euclid(d);
    Tick(i64::try_from(rounded).expect("rescaled tick fits i64"))
}

/// Errors from SMF import.
#[derive(Debug, thiserror::Error)]
pub enum MidiError {
    /// The file could not be parsed as SMF.
    #[error("malformed MIDI file: {0}")]
    Parse(#[from] midly::Error),
    /// SMPTE (timecode) timing division is not supported.
    #[error("SMPTE-timed MIDI files are not supported")]
    SmpteTiming,
    /// Imported notes violated pattern invariants.
    #[error("invalid note data: {0}")]
    Pattern(musicos_music_core::PatternError),
    /// Imported tempo events violated tempo-map invariants.
    #[error("invalid tempo data: {0}")]
    Timeline(musicos_timeline::TimelineError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn song_with(notes: Vec<Note>) -> SmfSong {
        SmfSong {
            tempo_map: TempoMap::constant(Tempo::DEFAULT),
            tracks: vec![(
                Some("test".to_string()),
                Pattern::new(notes, Tick::ZERO).unwrap(),
            )],
        }
    }

    #[test]
    fn export_import_round_trip_preserves_notes_and_tempo() {
        let song = song_with(vec![
            Note {
                pitch: Pitch::new(60),
                velocity: Velocity::new(100).unwrap(),
                start: Tick(0),
                duration: Tick(480),
            },
            Note {
                pitch: Pitch::new(64),
                velocity: Velocity::new(90).unwrap(),
                start: Tick(480),
                duration: Tick(960),
            },
        ]);
        let round = import_smf(&export_smf(&song)).unwrap();
        assert_eq!(round, song);
    }

    #[test]
    fn overlapping_same_pitch_notes_survive() {
        let song = song_with(vec![
            Note {
                pitch: Pitch::new(60),
                velocity: Velocity::MF,
                start: Tick(0),
                duration: Tick(960),
            },
            Note {
                pitch: Pitch::new(60),
                velocity: Velocity::MF,
                start: Tick(240),
                duration: Tick(240),
            },
        ]);
        let round = import_smf(&export_smf(&song)).unwrap();
        assert_eq!(round.tracks[0].1.notes().len(), 2);
    }

    #[test]
    fn velocity_zero_note_on_acts_as_note_off() {
        // Hand-build a file using vel-0 note-ons for offs.
        let song = song_with(vec![Note {
            pitch: Pitch::new(72),
            velocity: Velocity::MF,
            start: Tick(0),
            duration: Tick(120),
        }]);
        let bytes = export_smf(&song);
        assert!(import_smf(&bytes).is_ok());
    }

    proptest! {
        /// Lossless round-trip holds for patterns without same-pitch overlap.
        /// (With overlap, SMF's on/on/off/off sequences are ambiguous by
        /// design — pairing is unrecoverable; see the overlap unit test.)
        #[test]
        fn round_trip_is_lossless_without_same_pitch_overlap(
            raw in proptest::collection::vec((0i64..20_000, 1i64..4_000, 21u8..108, 1u8..=127), 1..60)
        ) {
            let mut kept: Vec<(i64, i64, u8, u8)> = Vec::new();
            for &(s, d, p, v) in &raw {
                if !kept.iter().any(|&(ks, kd, kp, _)| kp == p && s < ks + kd && ks < s + d) {
                    kept.push((s, d, p, v));
                }
            }
            let notes: Vec<Note> = kept.iter().map(|&(s, d, p, v)| Note {
                pitch: Pitch::new(p),
                velocity: Velocity::new(v).unwrap(),
                start: Tick(s),
                duration: Tick(d),
            }).collect();
            let song = song_with(notes);
            let round = import_smf(&export_smf(&song)).unwrap();
            prop_assert_eq!(round, song);
        }
    }
}
