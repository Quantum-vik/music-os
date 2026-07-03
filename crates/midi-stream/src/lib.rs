//! Live MIDI streaming: play a project's notes into DAW synths in real time
//! through a virtual MIDI port (docs/12 "DAW bridges").
//!
//! [`schedule`] flattens a project into a time-ordered, channel-assigned
//! event list (pure — fully unit-testable); [`stream`] walks that list on a
//! wall clock and sends it through [`midir`]. On macOS/Linux the port is
//! created virtually ("MusicOS Out" — enable the IAC driver on macOS);
//! on Windows connect to a loopMIDI port by name instead.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use musicos_core_types::Tick;
use musicos_project_model::{ProjectState, TrackKind};

/// One scheduled MIDI message.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    /// When to send, in seconds from stream start.
    pub at_seconds: f64,
    /// Raw 3-byte channel message (status, data1, data2).
    pub bytes: [u8; 3],
}

const NOTE_ON: u8 = 0x90;
const NOTE_OFF: u8 = 0x80;
/// Microsecond clock rate used for tick→seconds conversion.
const CLOCK_RATE: u32 = 1_000_000;

/// Flattens a project into time-ordered MIDI events starting at `start_bar`
/// (4/4). Track *i* plays on MIDI channel `i % 16`, so a DAW can route each
/// track to its own instrument. Note-offs of simultaneous events sort before
/// note-ons (standard retrigger-safe ordering).
pub fn schedule(state: &ProjectState, start_bar: u64) -> Vec<Event> {
    let start_tick = Tick(i64::try_from(start_bar).unwrap_or(0) * musicos_core_types::PPQ * 4);
    let offset = tick_seconds(state, start_tick);
    let mut events = Vec::new();
    for (index, track) in state
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Midi && !t.mix.muted)
        .enumerate()
    {
        #[allow(clippy::cast_possible_truncation)]
        let channel = (index % 16) as u8;
        for placement in &track.placements {
            let clip = &state.clips[&placement.clip];
            for note in clip.pattern.notes() {
                let on = placement.at + note.start;
                let off = placement.at + note.end();
                if off <= start_tick {
                    continue;
                }
                let key = note.pitch.note;
                if on >= start_tick {
                    events.push(Event {
                        at_seconds: tick_seconds(state, on) - offset,
                        bytes: [NOTE_ON | channel, key, note.velocity.get()],
                    });
                }
                events.push(Event {
                    at_seconds: tick_seconds(state, off) - offset,
                    bytes: [NOTE_OFF | channel, key, 0],
                });
            }
        }
    }
    // Stable order: time, then note-offs before note-ons at the same time.
    events.sort_by(|a, b| {
        a.at_seconds
            .total_cmp(&b.at_seconds)
            .then_with(|| (a.bytes[0] & 0xF0).cmp(&(b.bytes[0] & 0xF0)))
    });
    events
}

fn tick_seconds(state: &ProjectState, at: Tick) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let micros = state.tempo_map.tick_to_samples(at, CLOCK_RATE).max(0) as f64;
    micros / f64::from(CLOCK_RATE)
}

/// Where to send the stream.
#[derive(Debug, Clone)]
pub enum Output {
    /// Create a virtual port with this name (macOS/Linux).
    Virtual(String),
    /// Connect to an existing port whose name contains this string.
    Named(String),
}

/// Lists the names of available MIDI output ports.
///
/// # Errors
/// Fails if the platform MIDI system cannot be initialized.
pub fn output_ports() -> Result<Vec<String>, StreamError> {
    let out = midir::MidiOutput::new("MusicOS").map_err(|e| StreamError::Midi(e.to_string()))?;
    Ok(out
        .ports()
        .iter()
        .filter_map(|p| out.port_name(p).ok())
        .collect())
}

/// Streams a project's MIDI to the given output in real time, blocking until
/// done or `stop` becomes true. `on_progress(sent, total)` is called as
/// events go out. All notes are silenced on exit (all-notes-off per channel).
///
/// # Errors
/// Fails if the port cannot be created/found or a send fails.
pub fn stream(
    state: &ProjectState,
    start_bar: u64,
    output: &Output,
    stop: &AtomicBool,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<(), StreamError> {
    let events = schedule(state, start_bar);
    if events.is_empty() {
        return Err(StreamError::NothingToPlay);
    }
    let midi = midir::MidiOutput::new("MusicOS").map_err(|e| StreamError::Midi(e.to_string()))?;
    let mut connection = match output {
        Output::Virtual(name) => {
            #[cfg(unix)]
            {
                use midir::os::unix::VirtualOutput as _;
                midi.create_virtual(name)
                    .map_err(|e| StreamError::Midi(e.to_string()))?
            }
            #[cfg(not(unix))]
            {
                return Err(StreamError::Midi(format!(
                    "virtual ports are not supported on this platform; connect to a \
                     loopMIDI port with --port instead of '{name}'"
                )));
            }
        }
        Output::Named(fragment) => {
            let port = midi
                .ports()
                .into_iter()
                .find(|p| {
                    midi.port_name(p)
                        .is_ok_and(|n| n.to_lowercase().contains(&fragment.to_lowercase()))
                })
                .ok_or_else(|| StreamError::PortNotFound(fragment.clone()))?;
            midi.connect(&port, "musicos-stream")
                .map_err(|e| StreamError::Midi(e.to_string()))?
        }
    };

    let total = events.len();
    let started = Instant::now();
    let mut sent = 0usize;
    for event in &events {
        while started.elapsed().as_secs_f64() < event.at_seconds {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let remaining = event.at_seconds - started.elapsed().as_secs_f64();
            std::thread::sleep(Duration::from_secs_f64(remaining.clamp(0.0005, 0.005)));
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        connection
            .send(&event.bytes)
            .map_err(|e| StreamError::Midi(e.to_string()))?;
        sent += 1;
        on_progress(sent, total);
    }
    // Silence everything we may have left ringing (CC 123 all-notes-off).
    for channel in 0..16u8 {
        let _ = connection.send(&[0xB0 | channel, 123, 0]);
    }
    connection.close();
    Ok(())
}

/// Errors from MIDI streaming.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StreamError {
    /// The project has no notes to stream.
    #[error("project has no notes to stream")]
    NothingToPlay,
    /// No output port matched the requested name.
    #[error("no MIDI output port matching '{0}' (try --list-ports)")]
    PortNotFound(String),
    /// Platform MIDI failure.
    #[error("midi: {0}")]
    Midi(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicos_core_types::{Pitch, ProjectId, Velocity, PPQ};
    use musicos_music_core::{Note, Pattern};
    use musicos_project_model::Command;

    fn project() -> ProjectState {
        let mut s = ProjectState::new(ProjectId(1), "Stream");
        for name in ["Keys", "Bass"] {
            s.dispatch(Command::CreateTrack {
                name: name.into(),
                kind: TrackKind::Midi,
            })
            .unwrap();
        }
        for (track, pitch) in [(0u64, 60u8), (1u64, 36u8)] {
            let notes = vec![
                Note {
                    pitch: Pitch::new(pitch),
                    velocity: Velocity::clamped(100),
                    start: Tick(0),
                    duration: Tick(PPQ),
                },
                Note {
                    pitch: Pitch::new(pitch + 7),
                    velocity: Velocity::clamped(90),
                    start: Tick(PPQ),
                    duration: Tick(PPQ),
                },
            ];
            s.dispatch(Command::InsertClip {
                track: musicos_core_types::TrackId(track),
                name: format!("c{track}"),
                pattern: Pattern::new(notes, Tick(PPQ * 2)).unwrap(),
                at: Tick(0),
            })
            .unwrap();
        }
        s
    }

    #[test]
    fn schedule_is_ordered_paired_and_channel_assigned() {
        let state = project();
        let events = schedule(&state, 0);
        // 2 tracks x 2 notes x (on + off)
        assert_eq!(events.len(), 8);
        assert!(events
            .windows(2)
            .all(|w| w[0].at_seconds <= w[1].at_seconds));
        // Channels 0 and 1 both appear; ons and offs balance per channel.
        for channel in [0u8, 1] {
            let ons = events
                .iter()
                .filter(|e| e.bytes[0] == NOTE_ON | channel)
                .count();
            let offs = events
                .iter()
                .filter(|e| e.bytes[0] == NOTE_OFF | channel)
                .count();
            assert_eq!(ons, 2, "channel {channel} note-ons");
            assert_eq!(ons, offs, "channel {channel} balance");
        }
        // At 120 bpm one beat is 0.5 s: second note starts at 0.5.
        let second_on = events
            .iter()
            .find(|e| e.bytes[0] == NOTE_ON && e.bytes[1] == 67)
            .unwrap();
        assert!((second_on.at_seconds - 0.5).abs() < 1e-6);
    }

    #[test]
    fn schedule_seeks_and_clips_by_start_bar() {
        let state = project();
        // Bar 1 starts after both notes (which span 2 beats): nothing left.
        let events = schedule(&state, 1);
        assert!(events.is_empty());
        // Seeking to bar 0 keeps everything; a stop mid-note keeps its off.
        let all = schedule(&state, 0);
        assert_eq!(all.len(), 8);
    }

    #[test]
    fn muted_tracks_are_skipped() {
        let mut state = project();
        state
            .dispatch(Command::SetTrackMute {
                track: musicos_core_types::TrackId(1),
                muted: true,
            })
            .unwrap();
        let events = schedule(&state, 0);
        assert_eq!(events.len(), 4);
        assert!(
            events.iter().all(|e| e.bytes[0].trailing_zeros() >= 4),
            "only channel 0"
        );
    }
}
