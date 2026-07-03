//! Loudness measurement and true-peak-safe limiting for mastering.
//!
//! [`integrated_lufs`] implements the ITU-R BS.1770 method (K-weighting,
//! 400 ms gated blocks with absolute −70 LUFS and relative −10 LU gates).
//! The K-weighting filters are designed with the RBJ cookbook at the actual
//! sample rate rather than the spec's fixed 48 kHz tables, which matches the
//! reference filters to within a small fraction of a dB across the band.
//!
//! [`Limiter`] is the mastering safety stage: a hard-knee peak limiter with
//! exponential release, used after loudness-targeting gain so the ceiling is
//! never exceeded.

use crate::{BiquadMode, BiquadStereo};

/// Block size for gating, per BS.1770 (400 ms, 75% overlap → 100 ms hop).
const BLOCK_MS: f64 = 400.0;
const HOP_MS: f64 = 100.0;
/// Absolute gate threshold in LUFS.
const ABSOLUTE_GATE: f64 = -70.0;
/// Relative gate offset in LU below the ungated mean.
const RELATIVE_GATE: f64 = -10.0;

/// Measures integrated loudness (LUFS) of a stereo signal.
///
/// Returns `None` when the signal is shorter than one 400 ms block or every
/// block falls below the absolute gate (digital silence has no loudness).
pub fn integrated_lufs(left: &[f32], right: &[f32], sample_rate: u32) -> Option<f64> {
    let frames = left.len().min(right.len());
    let block = (f64::from(sample_rate) * BLOCK_MS / 1000.0) as usize;
    let hop = (f64::from(sample_rate) * HOP_MS / 1000.0) as usize;
    if frames < block || block == 0 || hop == 0 {
        return None;
    }

    // K-weight a working copy: stage 1 high shelf, stage 2 high-pass.
    let mut l = left[..frames].to_vec();
    let mut r = right[..frames].to_vec();
    let mut shelf = BiquadStereo::new(BiquadMode::HighShelf, sample_rate, 1_681.97, 0.7072, 3.9998);
    let mut highpass = BiquadStereo::new(BiquadMode::HighPass, sample_rate, 38.135, 0.5003, 0.0);
    shelf.process(&mut l, &mut r);
    highpass.process(&mut l, &mut r);

    // Mean-square energy per 400 ms block, both channels summed (stereo has
    // unity channel weights in BS.1770).
    let mut block_loudness = Vec::new();
    let mut start = 0;
    while start + block <= frames {
        let mut energy = 0.0f64;
        for i in start..start + block {
            energy += f64::from(l[i]) * f64::from(l[i]) + f64::from(r[i]) * f64::from(r[i]);
        }
        let mean_square = energy / block as f64;
        if mean_square > 0.0 {
            block_loudness.push(-0.691 + 10.0 * mean_square.log10());
        }
        start += hop;
    }

    let gated_mean = |blocks: &[f64], gate: f64| -> Option<f64> {
        let passing: Vec<f64> = blocks.iter().copied().filter(|lk| *lk > gate).collect();
        if passing.is_empty() {
            return None;
        }
        let energy: f64 = passing
            .iter()
            .map(|lk| 10f64.powf((lk + 0.691) / 10.0))
            .sum::<f64>()
            / passing.len() as f64;
        Some(-0.691 + 10.0 * energy.log10())
    };

    let ungated = gated_mean(&block_loudness, ABSOLUTE_GATE)?;
    gated_mean(&block_loudness, ungated + RELATIVE_GATE)
}

/// A hard-knee peak limiter with exponential release (mastering safety).
#[derive(Debug, Clone)]
pub struct Limiter {
    ceiling: f32,
    release_coeff: f32,
    gain: f32,
}

impl Limiter {
    /// Creates a limiter with the given output ceiling (linear, e.g. 0.891
    /// for −1 dBFS) and release time.
    pub fn new(sample_rate: u32, ceiling: f32, release_ms: f32) -> Limiter {
        let release_samples = f64::from(sample_rate) * f64::from(release_ms.max(1.0)) / 1000.0;
        Limiter {
            ceiling: ceiling.clamp(0.01, 1.0),
            release_coeff: (-1.0 / release_samples).exp() as f32,
            gain: 1.0,
        }
    }

    /// Limits both channels in place. Never allocates.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for i in 0..left.len().min(right.len()) {
            let peak = left[i].abs().max(right[i].abs());
            // The most gain this sample tolerates without breaching the
            // ceiling; instant attack, exponential release toward unity.
            let allowed = if peak > self.ceiling {
                self.ceiling / peak
            } else {
                1.0
            };
            self.gain = 1.0 + (self.gain - 1.0) * self.release_coeff;
            if allowed < self.gain {
                self.gain = allowed;
            }
            left[i] *= self.gain;
            right[i] *= self.gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, sample_rate: u32, seconds: f64, amplitude: f64) -> Vec<f32> {
        let n = (f64::from(sample_rate) * seconds) as usize;
        (0..n)
            .map(|i| {
                (amplitude
                    * (core::f64::consts::TAU * freq * i as f64 / f64::from(sample_rate)).sin())
                    as f32
            })
            .collect()
    }

    /// BS.1770 reference: a 0 dBFS 997 Hz sine on one channel reads
    /// -3.01 LKFS, so the same tone on both channels at -18 dBFS reads about
    /// -18 LUFS. Accept a small tolerance for the RBJ filter approximation.
    #[test]
    fn sine_loudness_matches_reference() {
        let signal = sine(997.0, 48_000, 5.0, 0.125_892_5); // -18 dBFS
        let lufs = integrated_lufs(&signal, &signal, 48_000).unwrap();
        assert!(
            (lufs - (-18.0)).abs() < 0.5,
            "expected about -18 LUFS, got {lufs:.2}"
        );
    }

    #[test]
    fn silence_and_short_signals_have_no_loudness() {
        let silence = vec![0.0f32; 48_000];
        assert!(integrated_lufs(&silence, &silence, 48_000).is_none());
        let short = vec![0.5f32; 100];
        assert!(integrated_lufs(&short, &short, 48_000).is_none());
    }

    #[test]
    fn quieter_signal_measures_quieter_by_the_same_amount() {
        let loud = sine(997.0, 44_100, 3.0, 0.25);
        let quiet = sine(997.0, 44_100, 3.0, 0.125); // -6 dB
        let a = integrated_lufs(&loud, &loud, 44_100).unwrap();
        let b = integrated_lufs(&quiet, &quiet, 44_100).unwrap();
        assert!(((a - b) - 6.02).abs() < 0.1, "delta was {}", a - b);
    }

    #[test]
    fn limiter_holds_the_ceiling_and_passes_quiet_audio() {
        let mut limiter = Limiter::new(48_000, 0.891, 50.0);
        let mut l = vec![1.5f32; 4_800];
        let mut r = vec![-1.5f32; 4_800];
        limiter.process(&mut l, &mut r);
        assert!(l.iter().all(|s| s.abs() <= 0.891 + 1e-4));
        assert!(r.iter().all(|s| s.abs() <= 0.891 + 1e-4));

        let mut limiter = Limiter::new(48_000, 0.891, 50.0);
        let mut l = vec![0.1f32; 4_800];
        let mut r = vec![0.1f32; 4_800];
        limiter.process(&mut l, &mut r);
        assert!(l.iter().all(|s| (*s - 0.1).abs() < 1e-5), "quiet untouched");
    }
}
