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
