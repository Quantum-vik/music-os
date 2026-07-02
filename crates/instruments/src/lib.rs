//! Built-in instruments: subtractive synthesizer and sampler.
//!
//! Phase 2 milestone 1 ships [`SimpleSynth`]: a deterministic polyphonic
//! synthesizer used to make projects audible with zero plugins installed.
//! It renders whole notes offline; the streaming voice-managed version that
//! runs inside the real-time graph (`docs/04` §5) lands with the engine.
//! The sampler follows in a later milestone.

use musicos_core_types::Pitch;

/// Oscillator waveforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Waveform {
    /// Band-unlimited sawtooth (bright; aliasing acceptable for v0).
    Saw,
    /// Pure sine.
    Sine,
    /// Square (50% duty).
    Square,
}

/// A deterministic offline synthesizer voice description.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimpleSynth {
    /// Oscillator waveform.
    pub waveform: Waveform,
    /// Attack time in seconds.
    pub attack: f32,
    /// Release time in seconds (applied after note-off).
    pub release: f32,
}

impl Default for SimpleSynth {
    fn default() -> Self {
        SimpleSynth {
            waveform: Waveform::Saw,
            attack: 0.005,
            release: 0.08,
        }
    }
}

impl SimpleSynth {
    /// Frequency of a pitch in Hz (12-TET, A4 = 440, cents applied).
    pub fn frequency(pitch: Pitch) -> f32 {
        let semis = f32::from(pitch.note) - 69.0 + f32::from(pitch.cents) / 100.0;
        440.0 * (semis / 12.0).exp2()
    }

    /// Total rendered length in samples for a note of `held` samples
    /// (the release tail extends past note-off).
    pub fn rendered_len(&self, held: usize, sample_rate: u32) -> usize {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )] // small positive times; sample rates << 2^24
        let tail = (self.release * sample_rate as f32).ceil() as usize;
        held + tail
    }

    /// Renders one note into a mono buffer: amplitude from `velocity_gain`
    /// (0..=1), envelope = linear attack, sustain, linear release after
    /// note-off. Purely functional and deterministic.
    pub fn render_note(
        &self,
        pitch: Pitch,
        velocity_gain: f32,
        held: usize,
        sample_rate: u32,
    ) -> Vec<f32> {
        let total = self.rendered_len(held, sample_rate);
        #[allow(clippy::cast_precision_loss)] // sample rates << 2^24
        let sr = sample_rate as f32;
        let freq = Self::frequency(pitch);
        let attack_samples = (self.attack * sr).max(1.0);
        let release_samples = (self.release * sr).max(1.0);

        let mut out = Vec::with_capacity(total);
        let mut phase: f32 = 0.0;
        let step = freq / sr;
        for i in 0..total {
            #[allow(clippy::cast_precision_loss)] // buffer indices << 2^24
            let i_f = i as f32;
            let env_attack = (i_f / attack_samples).min(1.0);
            let env_release = if i < held {
                1.0
            } else {
                #[allow(clippy::cast_precision_loss)]
                let past = (i - held) as f32;
                (1.0 - past / release_samples).max(0.0)
            };
            let sample = match self.waveform {
                Waveform::Saw => 2.0 * phase - 1.0,
                Waveform::Sine => (core::f32::consts::TAU * phase).sin(),
                Waveform::Square => {
                    if phase < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
            };
            out.push(sample * env_attack * env_release * velocity_gain * 0.3);
            phase += step;
            if phase >= 1.0 {
                phase -= 1.0;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tuning_is_correct() {
        assert!((SimpleSynth::frequency(Pitch::new(69)) - 440.0).abs() < 1e-3);
        assert!((SimpleSynth::frequency(Pitch::new(57)) - 220.0).abs() < 1e-3);
        let sharp = Pitch {
            note: 69,
            cents: 100,
        };
        assert!(
            (SimpleSynth::frequency(sharp) - SimpleSynth::frequency(Pitch::new(70))).abs() < 1e-3
        );
    }

    #[test]
    fn note_render_is_deterministic_and_bounded() {
        let synth = SimpleSynth::default();
        let a = synth.render_note(Pitch::new(60), 0.8, 4_800, 48_000);
        let b = synth.render_note(Pitch::new(60), 0.8, 4_800, 48_000);
        assert_eq!(a, b);
        assert_eq!(a.len(), synth.rendered_len(4_800, 48_000));
        assert!(a.iter().all(|s| s.abs() <= 1.0));
        assert!(a.iter().any(|s| s.abs() > 0.01), "must not be silent");
    }

    #[test]
    fn envelope_reaches_zero_at_the_end() {
        let synth = SimpleSynth::default();
        let buf = synth.render_note(Pitch::new(60), 1.0, 480, 48_000);
        assert!(buf.last().unwrap().abs() < 1e-3);
    }
}
