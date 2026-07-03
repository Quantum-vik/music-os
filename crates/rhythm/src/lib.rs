//! Rhythm engine: Euclidean rhythms, swing, and rhythmic analysis.
//!
//! Part of the MusicOS workspace; see `docs/05_Music_Core.md` §2 for the
//! rhythm engine spec and `docs/02_System_Architecture.md` for this crate's
//! place in the layer diagram. Euclidean rhythms distribute `pulses` onsets
//! as evenly as possible across `steps` slots (Bjorklund), covering a huge
//! family of world-music grooves — tresillo, cinquillo, and friends — with
//! one function.

use musicos_core_types::{Pitch, Tick, Velocity, PPQ};
use musicos_music_core::{Note, Pattern};

/// Generates a Euclidean (Bjorklund) rhythm: `pulses` onsets distributed
/// maximally evenly over `steps` slots, then rotated left by `rotation`
/// steps.
///
/// Edge cases: `pulses >= steps` yields all `true`; `pulses == 0` (or
/// `steps == 0`) yields all `false` (an empty vec for zero steps).
///
/// Classic results (rotation 0): `euclidean(8, 3, 0)` is the tresillo
/// `[x..x..x.]`, `euclidean(8, 4, 0)` alternates, and `euclidean(12, 5, 0)`
/// is `[x..x.x..x.x.]`.
pub fn euclidean(steps: usize, pulses: usize, rotation: usize) -> Vec<bool> {
    if steps == 0 {
        return Vec::new();
    }
    if pulses == 0 {
        return vec![false; steps];
    }
    if pulses >= steps {
        return vec![true; steps];
    }
    let base = bjorklund(steps, pulses);
    (0..steps).map(|i| base[(i + rotation) % steps]).collect()
}

/// The Bjorklund pairing algorithm: repeatedly folds the shorter group of
/// sequences into the longer one, like the Euclidean GCD algorithm.
/// Precondition: `0 < pulses < steps`.
fn bjorklund(steps: usize, pulses: usize) -> Vec<bool> {
    let mut heads: Vec<Vec<bool>> = vec![vec![true]; pulses];
    let mut tails: Vec<Vec<bool>> = vec![vec![false]; steps - pulses];
    while tails.len() > 1 {
        let n = heads.len().min(tails.len());
        let leftover = if heads.len() > n {
            heads.split_off(n)
        } else {
            tails.split_off(n)
        };
        for (head, tail) in heads.iter_mut().zip(tails) {
            head.extend(tail);
        }
        tails = leftover;
    }
    heads.into_iter().chain(tails).flatten().collect()
}

/// Renders a Euclidean rhythm as a [`Pattern`] of [`Note`]s.
///
/// Each `true` step becomes a note at `pitch = key` with the given
/// `velocity` (clamped into the valid 1..=127 MIDI range), starting at
/// `step_index * step_ticks` and lasting half a step (at least one tick).
/// The pattern length is `steps * step_ticks`.
///
/// # Panics
/// Panics if `steps` or `step_ticks.0` is large enough that
/// `steps * step_ticks` overflows an `i64` — unreachable for any musical
/// input.
pub fn euclidean_pattern(
    steps: usize,
    pulses: usize,
    rotation: usize,
    key: Pitch,
    step_ticks: Tick,
    velocity: u8,
) -> Pattern {
    let grid = euclidean(steps, pulses, rotation);
    let half_step = Tick((step_ticks.0 / 2).max(1));
    let velocity = Velocity::clamped(i32::from(velocity));
    let notes: Vec<Note> = grid
        .iter()
        .enumerate()
        .filter(|(_, &on)| on)
        .map(|(i, _)| {
            let index = i64::try_from(i).expect("step index fits in i64");
            Note {
                pitch: key,
                velocity,
                start: Tick(index * step_ticks.0),
                duration: half_step,
            }
        })
        .collect();
    let steps_i64 = i64::try_from(steps).expect("step count fits in i64");
    let length = Tick(steps_i64 * step_ticks.0);
    Pattern::new(notes, length).expect("euclidean notes are non-negative with positive duration")
}

/// Swing delay for a step: offbeat (odd-indexed) steps are delayed by
/// `amount` (0.0..=1.0) of half a step; even steps get [`Tick::ZERO`].
///
/// Integer tick math: the offset is `step_ticks.0 / 2` scaled by the clamped
/// `amount` and rounded to the nearest tick.
pub fn swing_offset(step_index: usize, step_ticks: Tick, amount: f32) -> Tick {
    if step_index % 2 == 0 {
        return Tick::ZERO;
    }
    let half = step_ticks.0 / 2;
    let amount = f64::from(amount.clamp(0.0, 1.0));
    Tick((half as f64 * amount).round() as i64)
}

/// Note density: notes per quarter note of pattern length.
///
/// Returns `0.0` for an empty or zero-length pattern.
pub fn density(pattern: &Pattern) -> f32 {
    if pattern.notes().is_empty() || pattern.length().0 <= 0 {
        return 0.0;
    }
    let quarters = pattern.length().0 as f64 / PPQ as f64;
    (pattern.notes().len() as f64 / quarters) as f32
}

/// Syncopation measure: the fraction of notes that do *not* start on an
/// eighth-note grid line (`start % (PPQ / 2) != 0`).
///
/// Returns `0.0` for an empty pattern.
pub fn syncopation(pattern: &Pattern) -> f32 {
    let notes = pattern.notes();
    if notes.is_empty() {
        return 0.0;
    }
    let eighth = PPQ / 2;
    let off_grid = notes.iter().filter(|n| n.start.0 % eighth != 0).count();
    (off_grid as f64 / notes.len() as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: bool = true;
    const F: bool = false;

    #[test]
    fn tresillo() {
        assert_eq!(euclidean(8, 3, 0), vec![T, F, F, T, F, F, T, F]);
    }

    #[test]
    fn four_on_eight_alternates() {
        assert_eq!(euclidean(8, 4, 0), vec![T, F, T, F, T, F, T, F]);
    }

    #[test]
    fn twelve_five() {
        assert_eq!(
            euclidean(12, 5, 0),
            vec![T, F, F, T, F, T, F, F, T, F, T, F]
        );
    }

    #[test]
    fn cinquillo() {
        assert_eq!(euclidean(8, 5, 0), vec![T, F, T, T, F, T, T, F]);
    }

    #[test]
    fn edge_cases() {
        assert_eq!(euclidean(4, 0, 0), vec![F, F, F, F]);
        assert_eq!(euclidean(4, 4, 0), vec![T, T, T, T]);
        assert_eq!(euclidean(4, 9, 0), vec![T, T, T, T]);
        assert!(euclidean(0, 3, 0).is_empty());
    }

    #[test]
    fn rotation_rotates_left() {
        let base = euclidean(8, 3, 0);
        let rotated = euclidean(8, 3, 3);
        for i in 0..8 {
            assert_eq!(rotated[i], base[(i + 3) % 8]);
        }
        // Full rotation is the identity.
        assert_eq!(euclidean(8, 3, 8), base);
    }

    #[test]
    fn pattern_renders_notes() {
        let step = Tick(PPQ / 2);
        let p = euclidean_pattern(8, 3, 0, Pitch::new(36), step, 100);
        assert_eq!(p.length(), Tick(8 * step.0));
        assert_eq!(p.notes().len(), 3);
        let starts: Vec<i64> = p.notes().iter().map(|n| n.start.0).collect();
        assert_eq!(starts, vec![0, 3 * step.0, 6 * step.0]);
        for n in p.notes() {
            assert_eq!(n.pitch, Pitch::new(36));
            assert_eq!(n.duration, Tick(step.0 / 2));
        }
    }

    #[test]
    fn pattern_velocity_is_clamped() {
        let p = euclidean_pattern(4, 1, 0, Pitch::new(60), Tick(PPQ), 0);
        assert_eq!(p.notes().len(), 1);
    }

    #[test]
    fn swing_even_steps_unmoved() {
        assert_eq!(swing_offset(0, Tick(PPQ), 0.5), Tick::ZERO);
        assert_eq!(swing_offset(2, Tick(PPQ), 1.0), Tick::ZERO);
    }

    #[test]
    fn swing_odd_steps_delayed() {
        // Half a step is PPQ/2 = 480 ticks; 50% swing delays by 240.
        assert_eq!(swing_offset(1, Tick(PPQ), 0.5), Tick(240));
        assert_eq!(swing_offset(3, Tick(PPQ), 1.0), Tick(480));
        assert_eq!(swing_offset(1, Tick(PPQ), 0.0), Tick::ZERO);
        // Out-of-range amounts clamp.
        assert_eq!(swing_offset(1, Tick(PPQ), 2.0), Tick(480));
        assert_eq!(swing_offset(1, Tick(PPQ), -1.0), Tick::ZERO);
    }

    fn note(start: i64, duration: i64) -> Note {
        Note {
            pitch: Pitch::new(60),
            velocity: Velocity::MF,
            start: Tick(start),
            duration: Tick(duration),
        }
    }

    #[test]
    fn density_counts_notes_per_quarter() {
        // 4 notes over 4 quarter notes = 1.0.
        let notes = (0..4).map(|i| note(i * PPQ, PPQ / 2)).collect();
        let p = Pattern::new(notes, Tick(4 * PPQ)).unwrap();
        assert!((density(&p) - 1.0).abs() < f32::EPSILON);
        // 8 notes over the same length = 2.0.
        let notes = (0..8).map(|i| note(i * PPQ / 2, PPQ / 4)).collect();
        let p = Pattern::new(notes, Tick(4 * PPQ)).unwrap();
        assert!((density(&p) - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn density_empty_is_zero() {
        assert!(density(&Pattern::empty(Tick(4 * PPQ))).abs() < f32::EPSILON);
        assert!(density(&Pattern::empty(Tick::ZERO)).abs() < f32::EPSILON);
    }

    #[test]
    fn syncopation_fraction_off_eighth_grid() {
        // Two on-grid notes, two off-grid (sixteenth offsets) = 0.5.
        let notes = vec![
            note(0, 100),
            note(PPQ / 2, 100),       // on the eighth grid
            note(PPQ / 4, 100),       // off
            note(PPQ + PPQ / 4, 100), // off
        ];
        let p = Pattern::new(notes, Tick(2 * PPQ)).unwrap();
        assert!((syncopation(&p) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn syncopation_empty_and_on_grid() {
        assert!(syncopation(&Pattern::empty(Tick(PPQ))).abs() < f32::EPSILON);
        let notes = (0..4).map(|i| note(i * PPQ / 2, 100)).collect();
        let p = Pattern::new(notes, Tick(2 * PPQ)).unwrap();
        assert!(syncopation(&p).abs() < f32::EPSILON);
    }
}
