//! Agentic FL Studio control over the MusicOS Bridge protocol.
//!
//! Commands are `SysEx` frames `F0 7D <cmd> <7-bit args> F7`, decoded by the
//! FL-side MIDI Controller Script (`integrations/fl-studio/`). Note data
//! travels as ordinary MIDI: [`FlBridge::record_song`] arms FL's recording,
//! streams the project, and stops — the notes land in FL's piano roll as
//! real, editable content. Inserting brand-new plugin instances is the one
//! thing FL does not expose to scripts; use a template project with your
//! instruments loaded, and the agent drives everything else.

use std::sync::atomic::AtomicBool;

use musicos_project_model::ProjectState;

use crate::{Output, StreamError};

const SYSEX_START: u8 = 0xF0;
/// Educational/experimental manufacturer id.
const MANUFACTURER: u8 = 0x7D;
const SYSEX_END: u8 = 0xF7;

const CMD_TRANSPORT: u8 = 0x01;
const CMD_TEMPO: u8 = 0x02;
const CMD_SELECT_PATTERN: u8 = 0x03;
const CMD_SELECT_CHANNEL: u8 = 0x04;
const CMD_MIXER_LEVEL: u8 = 0x05;
const CMD_PLUGIN_PARAM: u8 = 0x06;
const CMD_METRONOME: u8 = 0x07;

fn u14(value: u16) -> [u8; 2] {
    [(value & 0x7F) as u8, (value >> 7) as u8 & 0x7F]
}

/// Builds one bridge `SysEx` frame (exposed for protocol tests).
pub fn frame(cmd: u8, args: &[u8]) -> Vec<u8> {
    let mut bytes = vec![SYSEX_START, MANUFACTURER, cmd];
    bytes.extend_from_slice(args);
    bytes.push(SYSEX_END);
    bytes
}

/// Transport actions understood by the FL-side script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Stop playback/recording.
    Stop,
    /// Start playback.
    Play,
    /// Toggle record arm.
    Record,
}

/// A live connection to the FL-side bridge script.
pub struct FlBridge {
    connection: midir::MidiOutputConnection,
}

impl FlBridge {
    /// Connects to FL: a virtual "MusicOS Bridge" port by default, or an
    /// existing port whose name contains `port` (Windows/loopMIDI).
    ///
    /// # Errors
    /// Fails if the port cannot be created or found.
    pub fn connect(port: Option<&str>) -> Result<FlBridge, StreamError> {
        let midi = midir::MidiOutput::new("MusicOS Bridge")
            .map_err(|e| StreamError::Midi(e.to_string()))?;
        let connection = match port {
            Some(fragment) => {
                let found = midi
                    .ports()
                    .into_iter()
                    .find(|p| {
                        midi.port_name(p)
                            .is_ok_and(|n| n.to_lowercase().contains(&fragment.to_lowercase()))
                    })
                    .ok_or_else(|| StreamError::PortNotFound(fragment.to_string()))?;
                midi.connect(&found, "musicos-fl-bridge")
                    .map_err(|e| StreamError::Midi(e.to_string()))?
            }
            None => {
                #[cfg(unix)]
                {
                    use midir::os::unix::VirtualOutput as _;
                    midi.create_virtual("MusicOS Bridge")
                        .map_err(|e| StreamError::Midi(e.to_string()))?
                }
                #[cfg(not(unix))]
                {
                    return Err(StreamError::Midi(
                        "create a loopMIDI port and pass its name on Windows".into(),
                    ));
                }
            }
        };
        Ok(FlBridge { connection })
    }

    fn send(&mut self, bytes: &[u8]) -> Result<(), StreamError> {
        self.connection
            .send(bytes)
            .map_err(|e| StreamError::Midi(e.to_string()))
    }

    /// Sends a transport action.
    ///
    /// # Errors
    /// Fails on MIDI send errors.
    pub fn transport(&mut self, action: Transport) -> Result<(), StreamError> {
        let code = match action {
            Transport::Stop => 0,
            Transport::Play => 1,
            Transport::Record => 2,
        };
        self.send(&frame(CMD_TRANSPORT, &[code]))
    }

    /// Sets FL's tempo (clamped to 20..=999 bpm).
    ///
    /// # Errors
    /// Fails on MIDI send errors.
    pub fn set_tempo(&mut self, bpm: f64) -> Result<(), StreamError> {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let tenths = (bpm.clamp(20.0, 999.0) * 10.0).round() as u16;
        self.send(&frame(CMD_TEMPO, &u14(tenths)))
    }

    /// Selects a pattern (1-based, like FL's UI).
    ///
    /// # Errors
    /// Fails on MIDI send errors.
    pub fn select_pattern(&mut self, pattern: u16) -> Result<(), StreamError> {
        self.send(&frame(CMD_SELECT_PATTERN, &u14(pattern)))
    }

    /// Selects a channel-rack channel (0-based).
    ///
    /// # Errors
    /// Fails on MIDI send errors.
    pub fn select_channel(&mut self, channel: u16) -> Result<(), StreamError> {
        self.send(&frame(CMD_SELECT_CHANNEL, &u14(channel)))
    }

    /// Sets a mixer track volume (0.0..=1.0; 0.8 is FL's unity default).
    ///
    /// # Errors
    /// Fails on MIDI send errors.
    pub fn mixer_level(&mut self, track: u16, level: f32) -> Result<(), StreamError> {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let scaled = (f64::from(level.clamp(0.0, 1.0)) * 12_800.0).round() as u16;
        let mut args = u14(track).to_vec();
        args.extend_from_slice(&u14(scaled));
        self.send(&frame(CMD_MIXER_LEVEL, &args))
    }

    /// Sets a parameter (0.0..=1.0 normalized) on the plugin of a channel.
    ///
    /// # Errors
    /// Fails on MIDI send errors.
    pub fn plugin_param(
        &mut self,
        channel: u16,
        param: u16,
        value: f32,
    ) -> Result<(), StreamError> {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let scaled = (f64::from(value.clamp(0.0, 1.0)) * 16_383.0).round() as u16;
        let mut args = u14(channel).to_vec();
        args.extend_from_slice(&u14(param));
        args.extend_from_slice(&u14(scaled));
        self.send(&frame(CMD_PLUGIN_PARAM, &args))
    }

    /// Enables/disables FL's metronome.
    ///
    /// # Errors
    /// Fails on MIDI send errors.
    pub fn metronome(&mut self, on: bool) -> Result<(), StreamError> {
        self.send(&frame(CMD_METRONOME, &[u8::from(on)]))
    }

    /// The full agentic loop: set FL's tempo to the project's, arm
    /// recording, stream the song as live MIDI (notes land in FL's piano
    /// roll), then stop. Uses the same port for commands and notes.
    ///
    /// # Errors
    /// Fails on MIDI errors or an empty project.
    pub fn record_song(
        mut self,
        state: &ProjectState,
        start_bar: u64,
        stop: &AtomicBool,
        on_progress: impl FnMut(usize, usize),
    ) -> Result<(), StreamError> {
        let bpm = state
            .tempo_map
            .tempo_at(musicos_core_types::Tick::ZERO)
            .bpm();
        self.set_tempo(bpm)?;
        self.transport(Transport::Record)?;
        // The bridge port stays open; stream notes through a second channel
        // on the same connection by reusing the raw sender.
        let result = crate::stream_over(&mut self.connection, state, start_bar, stop, on_progress);
        let _ = self.transport(Transport::Stop);
        result
    }
}

/// The default output name the FL bridge exposes.
pub const BRIDGE_PORT: &str = "MusicOS Bridge";

/// Convenience: where note streaming should go when FL is the target.
pub fn bridge_output() -> Output {
    Output::Virtual(BRIDGE_PORT.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors integrations/fl-studio/test_protocol.py exactly.
    #[test]
    fn frames_match_the_fl_script_vectors() {
        assert_eq!(
            frame(CMD_TRANSPORT, &[1]),
            vec![0xF0, 0x7D, 0x01, 0x01, 0xF7]
        );
        assert_eq!(
            frame(CMD_TRANSPORT, &[0]),
            vec![0xF0, 0x7D, 0x01, 0x00, 0xF7]
        );
        assert_eq!(
            frame(CMD_TRANSPORT, &[2]),
            vec![0xF0, 0x7D, 0x01, 0x02, 0xF7]
        );
        let tempo = frame(CMD_TEMPO, &u14(925));
        assert_eq!(
            tempo,
            vec![
                0xF0,
                0x7D,
                0x02,
                (0x39D_u16 & 0x7F) as u8,
                (0x39D_u16 >> 7) as u8,
                0xF7
            ]
        );
        let mut args = u14(1).to_vec();
        args.extend_from_slice(&u14(9600));
        assert_eq!(
            frame(CMD_MIXER_LEVEL, &args),
            vec![
                0xF0,
                0x7D,
                0x05,
                1,
                0,
                (0x2580_u16 & 0x7F) as u8,
                (0x2580_u16 >> 7) as u8,
                0xF7
            ]
        );
        // Every argument byte stays 7-bit.
        for f in [frame(CMD_PLUGIN_PARAM, &[0x7F; 6]), tempo] {
            assert!(f[2..f.len() - 1].iter().all(|b| *b <= 0x7F));
        }
    }
}
