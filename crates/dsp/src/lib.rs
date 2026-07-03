//! DSP processors: EQ, dynamics, delay, reverb, and utility kernels.
//!
//! Phase 2 milestone 1 ships the utility layer: stereo buffers, gain/pan laws,
//! and peak normalization. The processor suite (EQ, dynamics, delay, reverb)
//! with the prepare/process contract from `docs/04` §6 lands with the graph
//! compiler. Everything here is pure and deterministic per platform.

/// An owned stereo buffer of `f32` frames (non-interleaved).
#[derive(Debug, Clone, PartialEq)]
pub struct StereoBuffer {
    /// Left channel samples.
    pub left: Vec<f32>,
    /// Right channel samples.
    pub right: Vec<f32>,
}

impl StereoBuffer {
    /// A silent buffer of `frames` samples per channel.
    pub fn silence(frames: usize) -> StereoBuffer {
        StereoBuffer {
            left: vec![0.0; frames],
            right: vec![0.0; frames],
        }
    }

    /// Samples per channel.
    pub fn frames(&self) -> usize {
        self.left.len()
    }

    /// Mixes `other` into `self` (sample-wise add). Buffers must be equal length.
    ///
    /// # Panics
    /// Panics if the buffers differ in length (a programming error upstream).
    pub fn mix_in(&mut self, other: &StereoBuffer) {
        assert_eq!(self.frames(), other.frames(), "buffer length mismatch");
        for (a, b) in self.left.iter_mut().zip(&other.left) {
            *a += b;
        }
        for (a, b) in self.right.iter_mut().zip(&other.right) {
            *a += b;
        }
    }

    /// Applies a linear gain to both channels.
    pub fn apply_gain(&mut self, gain: f32) {
        for s in self.left.iter_mut().chain(self.right.iter_mut()) {
            *s *= gain;
        }
    }

    /// The largest absolute sample value across both channels.
    pub fn peak(&self) -> f32 {
        self.left
            .iter()
            .chain(self.right.iter())
            .fold(0.0f32, |m, s| m.max(s.abs()))
    }

    /// Scales the buffer so its peak hits `target` (linear, e.g. 0.891 ≈ −1 dBFS).
    /// Buffers at or below the target (or silent) are left untouched — this
    /// only ever attenuates, so quiet material keeps its dynamics.
    pub fn limit_peak(&mut self, target: f32) {
        let peak = self.peak();
        if peak > target && peak > 0.0 {
            self.apply_gain(target / peak);
        }
    }

    /// Interleaves into `[L, R, L, R, …]` for encoders.
    pub fn interleave(&self) -> Vec<f32> {
        self.left
            .iter()
            .zip(&self.right)
            .flat_map(|(&l, &r)| [l, r])
            .collect()
    }
}

// --- Insert processors (docs/04 §6) ---------------------------------------------
//
// Stateful stereo processors with the prepare/process contract: construction
// allocates (delay lines, reverb networks); `process` never does. f32 samples
// with f64 filter state where error accumulates.

/// Biquad filter modes (RBJ audio-EQ cookbook).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BiquadMode {
    /// 12 dB/oct low-pass.
    LowPass,
    /// 12 dB/oct high-pass.
    HighPass,
    /// Peaking EQ (uses `gain_db`).
    Peak,
}

/// A stereo biquad filter (Direct Form I, f64 state).
#[derive(Debug, Clone)]
pub struct BiquadStereo {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    state: [[f64; 4]; 2], // per channel: x1, x2, y1, y2
}

impl BiquadStereo {
    /// Designs the filter. `q` is clamped to a stable range; `gain_db` only
    /// affects [`BiquadMode::Peak`].
    pub fn new(mode: BiquadMode, sample_rate: u32, freq_hz: f32, q: f32, gain_db: f32) -> Self {
        let fs = f64::from(sample_rate.max(1));
        let f = f64::from(freq_hz).clamp(10.0, fs * 0.49);
        let q = f64::from(q).clamp(0.1, 18.0);
        let w0 = core::f64::consts::TAU * f / fs;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a = 10f64.powf(f64::from(gain_db) / 40.0);

        let (b0, b1, b2, a0, a1, a2) = match mode {
            BiquadMode::LowPass => {
                let b1 = 1.0 - cos;
                (b1 / 2.0, b1, b1 / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
            }
            BiquadMode::HighPass => {
                let b1 = -(1.0 + cos);
                (
                    (1.0 + cos) / 2.0,
                    b1,
                    (1.0 + cos) / 2.0,
                    1.0 + alpha,
                    -2.0 * cos,
                    1.0 - alpha,
                )
            }
            BiquadMode::Peak => (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ),
        };
        BiquadStereo {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            state: [[0.0; 4]; 2],
        }
    }

    /// Filters both channels in place.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (ch, samples) in [left, right].into_iter().enumerate() {
            let [mut x1, mut x2, mut y1, mut y2] = self.state[ch];
            for s in samples.iter_mut() {
                let x = f64::from(*s);
                let y = self.b0 * x + self.b1 * x1 + self.b2 * x2 - self.a1 * y1 - self.a2 * y2;
                x2 = x1;
                x1 = x;
                y2 = y1;
                y1 = y;
                #[allow(clippy::cast_possible_truncation)]
                {
                    *s = y as f32;
                }
            }
            self.state[ch] = [x1, x2, y1, y2];
        }
    }
}

/// A stereo-linked peak compressor with attack/release envelope.
#[derive(Debug, Clone)]
pub struct Compressor {
    threshold_db: f32,
    ratio: f32,
    makeup: f32,
    attack_coef: f32,
    release_coef: f32,
    envelope: f32,
}

impl Compressor {
    /// Creates a compressor. `ratio` ≥ 1; times in milliseconds.
    pub fn new(
        sample_rate: u32,
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
        makeup_db: f32,
    ) -> Self {
        let coef = |ms: f32| {
            let samples = (ms.max(0.1) / 1000.0) * sample_rate.max(1) as f32;
            1.0 - (-1.0 / samples).exp()
        };
        Compressor {
            threshold_db,
            ratio: ratio.max(1.0),
            makeup: db_to_gain(makeup_db),
            attack_coef: coef(attack_ms),
            release_coef: coef(release_ms),
            envelope: 0.0,
        }
    }

    /// Compresses both channels in place (stereo-linked detection).
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for i in 0..left.len().min(right.len()) {
            let level = left[i].abs().max(right[i].abs());
            let coef = if level > self.envelope {
                self.attack_coef
            } else {
                self.release_coef
            };
            self.envelope += (level - self.envelope) * coef;
            let env_db = 20.0 * self.envelope.max(1e-6).log10();
            let over = env_db - self.threshold_db;
            let gain = if over > 0.0 {
                db_to_gain(-over * (1.0 - 1.0 / self.ratio))
            } else {
                1.0
            } * self.makeup;
            left[i] *= gain;
            right[i] *= gain;
        }
    }
}

/// A stereo feedback delay.
#[derive(Debug, Clone)]
pub struct StereoDelay {
    buffers: [Vec<f32>; 2],
    index: usize,
    feedback: f32,
    mix: f32,
}

impl StereoDelay {
    /// Creates a delay. `time_ms` up to 2000; `feedback` and `mix` in `[0, 1)`.
    pub fn new(sample_rate: u32, time_ms: f32, feedback: f32, mix: f32) -> Self {
        let len = ((time_ms.clamp(1.0, 2000.0) / 1000.0) * sample_rate.max(1) as f32)
            .round()
            .max(1.0) as usize;
        StereoDelay {
            buffers: [vec![0.0; len], vec![0.0; len]],
            index: 0,
            feedback: feedback.clamp(0.0, 0.95),
            mix: mix.clamp(0.0, 1.0),
        }
    }

    /// Applies the delay in place.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = self.buffers[0].len();
        for i in 0..left.len().min(right.len()) {
            for (ch, s) in [(0, &mut left[i]), (1, &mut right[i])] {
                let delayed = self.buffers[ch][self.index];
                self.buffers[ch][self.index] = (*s + delayed * self.feedback).clamp(-4.0, 4.0);
                *s = *s * (1.0 - self.mix) + delayed * self.mix;
            }
            self.index = (self.index + 1) % len;
        }
    }
}

/// A small Schroeder reverb (4 combs + 2 allpasses per channel, damped).
#[derive(Debug, Clone)]
pub struct Reverb {
    combs: Vec<Comb>,
    allpasses: Vec<Allpass>,
    mix: f32,
}

#[derive(Debug, Clone)]
struct Comb {
    buffer: Vec<f32>,
    index: usize,
    feedback: f32,
    damping: f32,
    filter: f32,
    channel: usize,
}

#[derive(Debug, Clone)]
struct Allpass {
    buffer: Vec<f32>,
    index: usize,
    channel: usize,
}

impl Reverb {
    /// Creates a reverb. `room` scales tail length; `damping` rolls off highs;
    /// `mix` is the wet fraction.
    pub fn new(sample_rate: u32, room: f32, damping: f32, mix: f32) -> Self {
        let scale = f64::from(sample_rate.max(1)) / 44_100.0;
        let sz = |n: usize| ((n as f64 * scale).round() as usize).max(1);
        let feedback = 0.7 + room.clamp(0.0, 1.0) * 0.28;
        let damping = damping.clamp(0.0, 1.0) * 0.4 + 0.2;
        let comb_tunings = [1116, 1188, 1277, 1356];
        let allpass_tunings = [556, 441];
        let mut combs = Vec::new();
        let mut allpasses = Vec::new();
        for channel in 0..2 {
            let spread = channel * 23;
            for n in comb_tunings {
                combs.push(Comb {
                    buffer: vec![0.0; sz(n + spread)],
                    index: 0,
                    feedback,
                    damping,
                    filter: 0.0,
                    channel,
                });
            }
            for n in allpass_tunings {
                allpasses.push(Allpass {
                    buffer: vec![0.0; sz(n + spread)],
                    index: 0,
                    channel,
                });
            }
        }
        Reverb {
            combs,
            allpasses,
            mix: mix.clamp(0.0, 1.0),
        }
    }

    /// Applies the reverb in place.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for i in 0..left.len().min(right.len()) {
            let dry = [left[i], right[i]];
            let input = (dry[0] + dry[1]) * 0.03;
            let mut wet = [0.0f32; 2];
            for comb in &mut self.combs {
                let out = comb.buffer[comb.index];
                comb.filter = out * (1.0 - comb.damping) + comb.filter * comb.damping;
                comb.buffer[comb.index] = input + comb.filter * comb.feedback;
                comb.index = (comb.index + 1) % comb.buffer.len();
                wet[comb.channel] += out;
            }
            for ap in &mut self.allpasses {
                let buffered = ap.buffer[ap.index];
                let out = -wet[ap.channel] + buffered;
                ap.buffer[ap.index] = wet[ap.channel] + buffered * 0.5;
                ap.index = (ap.index + 1) % ap.buffer.len();
                wet[ap.channel] = out;
            }
            left[i] = dry[0] * (1.0 - self.mix) + wet[0] * self.mix;
            right[i] = dry[1] * (1.0 - self.mix) + wet[1] * self.mix;
        }
    }
}

#[cfg(test)]
mod processor_tests {
    use super::*;

    fn impulse(n: usize) -> (Vec<f32>, Vec<f32>) {
        let mut l = vec![0.0; n];
        let r = vec![0.0; n];
        l[0] = 1.0;
        (l, r)
    }

    fn sine(freq: f32, sample_rate: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (core::f32::consts::TAU * freq * i as f32 / sample_rate as f32).sin())
            .collect()
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    }

    #[test]
    fn lowpass_attenuates_highs_and_passes_lows() {
        let sr = 48_000;
        let mut low = sine(100.0, sr, 4800);
        let mut low_r = low.clone();
        BiquadStereo::new(BiquadMode::LowPass, sr, 1000.0, 0.707, 0.0)
            .process(&mut low, &mut low_r);
        let mut high = sine(8000.0, sr, 4800);
        let mut high_r = high.clone();
        BiquadStereo::new(BiquadMode::LowPass, sr, 1000.0, 0.707, 0.0)
            .process(&mut high, &mut high_r);
        assert!(rms(&low) > 0.6, "low band passes ({})", rms(&low));
        assert!(rms(&high) < 0.1, "high band attenuated ({})", rms(&high));
    }

    #[test]
    fn compressor_reduces_loud_signals_more_than_quiet() {
        let sr = 48_000;
        let mut loud = vec![0.9f32; 4800];
        let mut loud_r = loud.clone();
        Compressor::new(sr, -20.0, 4.0, 1.0, 50.0, 0.0).process(&mut loud, &mut loud_r);
        let tail = &loud[2400..];
        assert!(rms(tail) < 0.45, "loud signal compressed ({})", rms(tail));

        let mut quiet = vec![0.05f32; 4800];
        let mut quiet_r = quiet.clone();
        Compressor::new(sr, -20.0, 4.0, 1.0, 50.0, 0.0).process(&mut quiet, &mut quiet_r);
        let qt = rms(&quiet[2400..]);
        assert!((qt - 0.05).abs() < 0.005, "quiet signal untouched ({qt})");
    }

    #[test]
    fn delay_echoes_at_the_right_offset() {
        let sr = 48_000;
        let (mut l, mut r) = impulse(sr as usize / 2);
        StereoDelay::new(sr, 100.0, 0.0, 1.0).process(&mut l, &mut r);
        let expected = 4800; // 100 ms at 48 kHz
        let peak = l
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap();
        assert_eq!(peak.0, expected, "echo lands 100 ms after the impulse");
    }

    #[test]
    fn reverb_produces_a_decaying_tail() {
        let sr = 48_000;
        let (mut l, mut r) = impulse(sr as usize);
        Reverb::new(sr, 0.8, 0.3, 1.0).process(&mut l, &mut r);
        let early = rms(&l[2400..9600]);
        let late = rms(&l[38_400..48_000]);
        assert!(early > 0.0005, "tail exists ({early})");
        assert!(late < early, "tail decays ({late} < {early})");
    }
}

/// Converts decibels to linear gain.
pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Equal-power pan law. `pan` in `[-1, 1]` (left to right) → `(left, right)` gains.
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    let angle = (pan + 1.0) * core::f32::consts::FRAC_PI_4;
    (angle.cos(), angle.sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_and_gain_are_samplewise() {
        let mut a = StereoBuffer::silence(4);
        let mut b = StereoBuffer::silence(4);
        b.left[0] = 0.5;
        b.right[3] = -0.25;
        a.mix_in(&b);
        a.apply_gain(2.0);
        assert!((a.left[0] - 1.0).abs() < 1e-6);
        assert!((a.right[3] + 0.5).abs() < 1e-6);
    }

    #[test]
    fn limit_peak_only_attenuates() {
        let mut hot = StereoBuffer::silence(2);
        hot.left[0] = 2.0;
        hot.limit_peak(1.0);
        assert!((hot.peak() - 1.0).abs() < 1e-6);

        let mut quiet = StereoBuffer::silence(2);
        quiet.left[0] = 0.1;
        quiet.limit_peak(1.0);
        assert!((quiet.left[0] - 0.1).abs() < f32::EPSILON); // untouched
    }

    #[test]
    fn pan_law_is_equal_power() {
        let (l, r) = pan_gains(0.0);
        assert!((l - r).abs() < 1e-6);
        assert!((l.mul_add(l, r * r) - 1.0).abs() < 1e-6);
        let (l, r) = pan_gains(-1.0);
        assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
    }

    #[test]
    fn db_conversions() {
        assert!((db_to_gain(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_gain(-6.0) - 0.501_19).abs() < 1e-4);
    }
}
