//! Built-in instruments: subtractive synthesizer and sampler.
//!
//! Phase 2 milestone 1 ships [`SimpleSynth`]: a deterministic polyphonic
//! synthesizer used to make projects audible with zero plugins installed.
//! It renders whole notes offline; the streaming voice-managed version that
//! runs inside the real-time graph (`docs/04` §5) lands with the engine.
//! The sampler follows in a later milestone.

pub mod soundfont;

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

// --- Streaming engine (docs/04 §5) ----------------------------------------------

/// Maximum simultaneous voices per streaming synth (fixed pool; stealing
/// beyond this — docs/04 §5).
pub const MAX_VOICES: usize = 32;

/// A scheduled note at sample resolution (produced by graph compilation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteEvent {
    /// Absolute frame the note starts on.
    pub start_frame: u64,
    /// Absolute frame the note is released on.
    pub end_frame: u64,
    /// Pitch.
    pub pitch: Pitch,
    /// Amplitude 0..=1 (velocity mapped).
    pub gain: f32,
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    step: f32,
    phase: f32,
    gain: f32,
    held_frames: u64,
    frames_done: u64,
    attack_frames: f32,
    release_frames: f32,
}

impl Voice {
    fn tick(&mut self, waveform: Waveform) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let t = self.frames_done as f32;
        let env_attack = (t / self.attack_frames).min(1.0);
        let env_release = if self.frames_done < self.held_frames {
            1.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            let past = (self.frames_done - self.held_frames) as f32;
            (1.0 - past / self.release_frames).max(0.0)
        };
        let sample = match waveform {
            Waveform::Saw => 2.0 * self.phase - 1.0,
            Waveform::Sine => (core::f32::consts::TAU * self.phase).sin(),
            Waveform::Square => {
                if self.phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        };
        self.phase += self.step;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        self.frames_done += 1;
        sample * env_attack * env_release * self.gain * 0.3
    }

    fn finished(&self) -> bool {
        #[allow(clippy::cast_precision_loss)]
        let past = self.frames_done.saturating_sub(self.held_frames) as f32;
        self.frames_done >= self.held_frames && past >= self.release_frames
    }
}

/// A streaming polyphonic synthesizer: consumes a sample-accurate
/// [`NoteEvent`] schedule and produces mono audio block by block with a fixed
/// voice pool. Monotonic playback is the fast path; a non-monotonic
/// `frame_offset` performs a seek (voices cleared, cursor repositioned).
#[derive(Debug, Clone)]
pub struct StreamingSynth {
    synth: SimpleSynth,
    sample_rate: u32,
    events: Vec<NoteEvent>,
    cursor: usize,
    next_frame: u64,
    voices: Vec<Voice>,
}

impl StreamingSynth {
    /// Creates a streaming synth over a schedule (sorted internally).
    pub fn new(synth: SimpleSynth, sample_rate: u32, mut events: Vec<NoteEvent>) -> Self {
        events.sort_by_key(|e| e.start_frame);
        StreamingSynth {
            synth,
            sample_rate,
            events,
            cursor: 0,
            next_frame: 0,
            voices: Vec::with_capacity(MAX_VOICES),
        }
    }

    /// Repositions playback to `frame` (voices cut, schedule cursor moved).
    pub fn seek(&mut self, frame: u64) {
        self.voices.clear();
        self.cursor = self.events.partition_point(|e| e.start_frame < frame);
        self.next_frame = frame;
    }

    /// Synthesizes one mono block starting at absolute `frame_offset`.
    pub fn process(&mut self, frame_offset: u64, out: &mut [f32]) {
        if frame_offset != self.next_frame {
            self.seek(frame_offset);
        }
        #[allow(clippy::cast_precision_loss)]
        let sr = self.sample_rate.max(1) as f32;
        let attack_frames = (self.synth.attack * sr).max(1.0);
        let release_frames = (self.synth.release * sr).max(1.0);

        for (i, slot) in out.iter_mut().enumerate() {
            let frame = frame_offset + i as u64;
            // Trigger every event scheduled for this exact frame.
            while self.cursor < self.events.len() && self.events[self.cursor].start_frame == frame {
                let event = self.events[self.cursor];
                self.cursor += 1;
                let voice = Voice {
                    step: SimpleSynth::frequency(event.pitch) / sr,
                    phase: 0.0,
                    gain: event.gain,
                    held_frames: event.end_frame.saturating_sub(event.start_frame).max(1),
                    frames_done: 0,
                    attack_frames,
                    release_frames,
                };
                if self.voices.len() < MAX_VOICES {
                    self.voices.push(voice);
                } else if let Some(steal) = self.steal_index() {
                    self.voices[steal] = voice;
                }
            }
            let mut acc = 0.0;
            for voice in &mut self.voices {
                acc += voice.tick(self.synth.waveform);
            }
            self.voices.retain(|v| !v.finished());
            *slot = acc;
        }
        self.next_frame = frame_offset + out.len() as u64;
    }

    /// Steal policy: the most-finished releasing voice, else the oldest.
    fn steal_index(&self) -> Option<usize> {
        self.voices
            .iter()
            .enumerate()
            .max_by_key(|(_, v)| (v.frames_done >= v.held_frames, v.frames_done))
            .map(|(i, _)| i)
    }

    /// Frames of release tail after the last event ends.
    pub fn tail_frames(&self) -> u64 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let tail = (self.synth.release * self.sample_rate.max(1) as f32).ceil() as u64;
        tail
    }
}

#[cfg(test)]
mod streaming_tests {
    use super::*;

    fn schedule() -> Vec<NoteEvent> {
        vec![
            NoteEvent {
                start_frame: 0,
                end_frame: 4800,
                pitch: Pitch::new(60),
                gain: 0.8,
            },
            NoteEvent {
                start_frame: 2400,
                end_frame: 7200,
                pitch: Pitch::new(64),
                gain: 0.8,
            },
        ]
    }

    #[test]
    fn chunking_is_invariant() {
        // One big buffer must equal many small blocks — the streaming contract.
        let mut a = StreamingSynth::new(SimpleSynth::default(), 48_000, schedule());
        let mut big = vec![0.0f32; 9600];
        a.process(0, &mut big);

        let mut b = StreamingSynth::new(SimpleSynth::default(), 48_000, schedule());
        let mut small = vec![0.0f32; 9600];
        for (i, chunk) in small.chunks_mut(512).enumerate() {
            b.process(i as u64 * 512, chunk);
        }
        assert_eq!(big, small);
    }

    #[test]
    fn voice_pool_is_bounded_under_burst() {
        let burst: Vec<NoteEvent> = (0..100)
            .map(|n| NoteEvent {
                start_frame: 0,
                end_frame: 48_000,
                pitch: Pitch::new(30 + (n % 60)),
                gain: 0.5,
            })
            .collect();
        let mut synth = StreamingSynth::new(SimpleSynth::default(), 48_000, burst);
        let mut out = vec![0.0f32; 512];
        synth.process(0, &mut out);
        assert!(synth.voices.len() <= MAX_VOICES);
        assert!(out.iter().any(|s| s.abs() > 0.01));
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn release_tail_rings_past_note_end_then_dies() {
        let events = vec![NoteEvent {
            start_frame: 0,
            end_frame: 1000,
            pitch: Pitch::new(69),
            gain: 1.0,
        }];
        let mut synth = StreamingSynth::new(SimpleSynth::default(), 48_000, events);
        let mut out = vec![0.0f32; 8000];
        synth.process(0, &mut out);
        assert!(out[1500].abs() > 0.0, "release tail after note-off");
        assert!(out[7500].abs() < 1e-6, "silent after the tail");
    }

    #[test]
    fn seek_repositions_and_stays_deterministic() {
        let mut a = StreamingSynth::new(SimpleSynth::default(), 48_000, schedule());
        let mut from_seek = vec![0.0f32; 512];
        a.process(2400, &mut from_seek); // non-monotonic entry = seek
                                         // A fresh synth seeked to the same place produces the same audio.
        let mut b = StreamingSynth::new(SimpleSynth::default(), 48_000, schedule());
        b.seek(2400);
        let mut fresh = vec![0.0f32; 512];
        b.process(2400, &mut fresh);
        assert_eq!(from_seek, fresh);
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
