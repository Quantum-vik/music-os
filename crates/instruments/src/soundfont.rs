//! Real instrument sounds via `SoundFont` (SF2) sampling.
//!
//! [`SoundBank`] wraps a General MIDI soundfont; [`SoundBank::render_track`]
//! renders one track's notes with a chosen GM program (0–127, or
//! [`PERCUSSION`] for the drum kit on MIDI channel 10). Rendering is
//! deterministic for identical inputs, preserving NFR-4. When no soundfont
//! is installed, callers fall back to the built-in `SimpleSynth`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};

/// Pseudo-program selecting the General MIDI percussion channel.
pub const PERCUSSION: u8 = 128;

/// One note for the sampler, in absolute sample positions.
#[derive(Debug, Clone, Copy)]
pub struct SampledNote {
    /// Note-on position (samples).
    pub on: usize,
    /// Note-off position (samples).
    pub off: usize,
    /// MIDI key.
    pub key: u8,
    /// MIDI velocity (1–127).
    pub velocity: u8,
}

/// A loaded soundfont, shareable across track renders.
pub struct SoundBank {
    font: Arc<SoundFont>,
}

impl SoundBank {
    /// Loads an SF2 file.
    ///
    /// # Errors
    /// Fails when the file is missing or not a valid soundfont.
    pub fn load(path: &Path) -> Result<SoundBank, String> {
        let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let font = SoundFont::new(&mut file).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(SoundBank {
            font: Arc::new(font),
        })
    }

    /// The user's installed soundfont, if any: `MUSICOS_SOUNDFONT` env var,
    /// else `~/.musicos/soundfont.sf2`.
    pub fn default_path() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("MUSICOS_SOUNDFONT") {
            let p = PathBuf::from(p);
            if p.is_file() {
                return Some(p);
            }
        }
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        let p = PathBuf::from(home).join(".musicos/soundfont.sf2");
        p.is_file().then_some(p)
    }

    /// Loads the default soundfont when one is installed.
    pub fn load_default() -> Option<SoundBank> {
        let path = Self::default_path()?;
        match Self::load(&path) {
            Ok(bank) => Some(bank),
            Err(e) => {
                eprintln!("MusicOS: soundfont unusable, using built-in synth: {e}");
                None
            }
        }
    }

    /// Renders a track's notes to mono at `sample_rate`, `len` samples long.
    /// `program` is the GM program (or [`PERCUSSION`] for drums).
    pub fn render_track(
        &self,
        sample_rate: u32,
        program: u8,
        notes: &[SampledNote],
        len: usize,
    ) -> Vec<f32> {
        #[allow(clippy::cast_possible_wrap)]
        let settings = SynthesizerSettings::new(sample_rate as i32);
        let Ok(mut synth) = Synthesizer::new(&self.font, &settings) else {
            return vec![0.0; len];
        };
        let channel = if program == PERCUSSION { 9 } else { 0 };
        if program != PERCUSSION {
            synth.process_midi_message(channel, 0xC0, i32::from(program), 0);
        }

        let mut events: Vec<(usize, bool, u8, u8)> = Vec::with_capacity(notes.len() * 2);
        for n in notes {
            events.push((n.on, true, n.key, n.velocity.clamp(1, 127)));
            events.push((n.off.max(n.on + 1), false, n.key, 0));
        }
        events.sort_by_key(|(at, on, ..)| (*at, u8::from(*on)));

        let mut mono = vec![0.0f32; len];
        let mut left = vec![0.0f32; 256];
        let mut right = vec![0.0f32; 256];
        let mut cursor = 0usize;
        let mut next_event = 0usize;
        while cursor < len {
            while next_event < events.len() && events[next_event].0 <= cursor {
                let (_, on, key, vel) = events[next_event];
                if on {
                    synth.note_on(channel, i32::from(key), i32::from(vel));
                } else {
                    synth.note_off(channel, i32::from(key));
                }
                next_event += 1;
            }
            let until_event = events
                .get(next_event)
                .map_or(len, |(at, ..)| (*at).min(len));
            let block = (until_event - cursor).clamp(1, 256);
            synth.render(&mut left[..block], &mut right[..block]);
            for i in 0..block {
                mono[cursor + i] = (left[i] + right[i]) * 0.5;
            }
            cursor += block;
        }
        mono
    }
}

/// Maps a human instrument name to a GM program (or [`PERCUSSION`]).
/// Accepts a bare number ("25") too. Returns `None` for unknown names.
pub fn program_for_name(name: &str) -> Option<u8> {
    let n = name.trim().to_lowercase();
    if let Ok(num) = n.parse::<u8>() {
        return (num <= PERCUSSION).then_some(num);
    }
    let program = match n.as_str() {
        "piano" | "grand piano" | "acoustic piano" => 0,
        "bright piano" => 1,
        "electric piano" | "epiano" | "rhodes" => 4,
        "harpsichord" => 6,
        "celesta" => 8,
        "music box" => 10,
        "vibraphone" => 11,
        "marimba" => 12,
        "organ" | "hammond" => 16,
        "church organ" => 19,
        "accordion" => 21,
        "harmonica" => 22,
        "guitar" | "acoustic guitar" | "steel guitar" => 25,
        "nylon guitar" | "classical guitar" => 24,
        "jazz guitar" | "electric guitar clean" => 26,
        "electric guitar" | "clean guitar" => 27,
        "muted guitar" => 28,
        "overdrive guitar" => 29,
        "distortion guitar" | "distorted guitar" => 30,
        "bass" | "acoustic bass" | "upright bass" => 32,
        "fingered bass" | "electric bass" => 33,
        "picked bass" => 34,
        "fretless bass" => 35,
        "slap bass" => 36,
        "synth bass" => 38,
        "violin" => 40,
        "viola" => 41,
        "cello" => 42,
        "harp" => 46,
        "strings" | "string ensemble" => 48,
        "slow strings" => 49,
        "synth strings" => 50,
        "choir" | "voice" | "aahs" => 52,
        "trumpet" => 56,
        "trombone" => 57,
        "tuba" => 58,
        "french horn" => 60,
        "brass" | "brass section" => 61,
        "soprano sax" => 64,
        "sax" | "alto sax" | "saxophone" => 65,
        "tenor sax" => 66,
        "oboe" => 68,
        "clarinet" => 71,
        "piccolo" => 72,
        "flute" => 73,
        "pan flute" => 75,
        "lead" | "synth lead" | "square lead" => 80,
        "saw lead" => 81,
        "pad" | "synth pad" | "warm pad" => 89,
        "new age pad" => 88,
        "bells" | "tubular bells" => 14,
        "sitar" => 104,
        "banjo" => 105,
        "kalimba" => 108,
        "steel drums" => 114,
        "drums" | "drum kit" | "percussion" | "kit" => PERCUSSION,
        _ => return None,
    };
    Some(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map_to_gm_programs() {
        assert_eq!(program_for_name("guitar"), Some(25));
        assert_eq!(program_for_name("Electric Guitar"), Some(27));
        assert_eq!(program_for_name("piano"), Some(0));
        assert_eq!(program_for_name("drums"), Some(PERCUSSION));
        assert_eq!(program_for_name("33"), Some(33));
        assert_eq!(program_for_name("kazoo-orchestra"), None);
        assert_eq!(program_for_name("200"), None);
    }

    /// Renders with a real soundfont when one is installed (dev machines);
    /// silently skipped otherwise so CI stays hermetic.
    #[test]
    fn sampler_renders_nonsilent_audio_when_soundfont_installed() {
        let Some(bank) = SoundBank::load_default() else {
            return;
        };
        let notes = [SampledNote {
            on: 0,
            off: 24_000,
            key: 60,
            velocity: 100,
        }];
        let mono = bank.render_track(48_000, 25, &notes, 48_000);
        let energy: f64 = mono.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
        assert!(energy > 1e-4, "soundfont produced silence");
        assert!(mono.iter().all(|s| s.is_finite()));
    }
}
