//! Offline render pipeline and audio export.
//!
//! Phase 2 milestone 1: a direct project renderer — every MIDI clip is
//! synthesized with [`SimpleSynth`], placed at its tempo-mapped sample
//! position, mixed, peak-limited, and written as 16-bit WAV. Renders are
//! deterministic per platform (NFR-4; see `docs/04` §6 for the honest limits).
//! The graph compiler / node executor from `docs/04` §3 replaces the direct
//! loop in the next milestone; the WAV encoder stays behind this crate either way.

use std::path::Path;

use musicos_core_types::Tick;
use musicos_dsp::{pan_gains, StereoBuffer};
use musicos_instruments::SimpleSynth;
use musicos_project_model::{ProjectState, TrackKind};

/// Render parameters.
#[derive(Debug, Clone, Copy)]
pub struct RenderOptions {
    /// Output sample rate in Hz.
    pub sample_rate: u32,
    /// Extra tail after the last note, in seconds.
    pub tail_seconds: f32,
    /// Peak ceiling as linear gain (0.891 ≈ −1 dBFS).
    pub peak_ceiling: f32,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            sample_rate: 48_000,
            tail_seconds: 0.5,
            peak_ceiling: 0.891,
        }
    }
}

/// Renders a project to a stereo buffer.
///
/// # Errors
/// Returns [`RenderError::EmptyProject`] if no MIDI clip contains notes.
pub fn render_project(
    state: &ProjectState,
    opts: &RenderOptions,
) -> Result<StereoBuffer, RenderError> {
    let synth = SimpleSynth::default();
    let sr = opts.sample_rate;

    // Determine total length: last note end across all placements + tail.
    let mut last_end_samples: usize = 0;
    let mut any_notes = false;
    for track in state.tracks.iter().filter(|t| t.kind == TrackKind::Midi) {
        for placement in &track.placements {
            let clip = &state.clips[&placement.clip];
            for note in clip.pattern.notes() {
                any_notes = true;
                let end_tick = placement.at + note.end();
                let end = sample_at(state, end_tick, sr) + synth.rendered_len(0, sr);
                last_end_samples = last_end_samples.max(end);
            }
        }
    }
    if !any_notes {
        return Err(RenderError::EmptyProject);
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // small positive time
    let tail = (opts.tail_seconds * sr as f32).ceil() as usize;
    let total = last_end_samples + tail;

    let mut master = StereoBuffer::silence(total);
    for track in state.tracks.iter().filter(|t| t.kind == TrackKind::Midi) {
        // v0: all tracks centered at unity; per-track gain/pan arrive with the
        // mix model (docs/03 ChannelStrip) in the next milestone.
        let (gl, gr) = pan_gains(0.0);
        for placement in &track.placements {
            let clip = &state.clips[&placement.clip];
            for note in clip.pattern.notes() {
                let start = sample_at(state, placement.at + note.start, sr);
                let end = sample_at(state, placement.at + note.end(), sr);
                let held = end.saturating_sub(start).max(1);
                let gain = f32::from(note.velocity.get()) / 127.0;
                let mono = synth.render_note(note.pitch, gain, held, sr);
                for (i, s) in mono.iter().enumerate() {
                    let at = start + i;
                    if at >= total {
                        break;
                    }
                    master.left[at] += s * gl;
                    master.right[at] += s * gr;
                }
            }
        }
    }
    master.limit_peak(opts.peak_ceiling);
    Ok(master)
}

/// Renders a project and writes a 16-bit stereo WAV file.
///
/// # Errors
/// Fails on empty projects or I/O errors.
pub fn render_to_wav(
    state: &ProjectState,
    opts: &RenderOptions,
    path: &Path,
) -> Result<RenderReport, RenderError> {
    let buffer = render_project(state, opts)?;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: opts.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for s in buffer.interleave() {
        #[allow(clippy::cast_possible_truncation)] // clamped to i16 range first
        writer.write_sample((s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)?;
    }
    writer.finalize()?;
    #[allow(clippy::cast_precision_loss)] // display only
    let seconds = buffer.frames() as f64 / f64::from(opts.sample_rate);
    Ok(RenderReport {
        frames: buffer.frames(),
        seconds,
        peak: buffer.peak(),
    })
}

/// What a render produced.
#[derive(Debug, Clone, Copy)]
pub struct RenderReport {
    /// Frames per channel written.
    pub frames: usize,
    /// Duration in seconds.
    pub seconds: f64,
    /// Peak level after limiting (linear).
    pub peak: f32,
}

fn sample_at(state: &ProjectState, tick: Tick, sample_rate: u32) -> usize {
    usize::try_from(state.tempo_map.tick_to_samples(tick, sample_rate).max(0))
        .expect("sample position fits usize")
}

/// Errors from rendering.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RenderError {
    /// The project has no MIDI notes to render.
    #[error("project has no notes to render")]
    EmptyProject,
    /// WAV encoding failure.
    #[error("wav: {0}")]
    Wav(#[from] hound::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicos_core_types::{Pitch, ProjectId, Tick, Velocity, PPQ};
    use musicos_music_core::{Note, Pattern};
    use musicos_project_model::Command;

    fn demo_project() -> ProjectState {
        let mut s = ProjectState::new(ProjectId(1), "Render");
        s.dispatch(Command::CreateTrack {
            name: "Lead".into(),
            kind: TrackKind::Midi,
        })
        .unwrap();
        let notes = vec![
            Note {
                pitch: Pitch::new(60),
                velocity: Velocity::MF,
                start: Tick::ZERO,
                duration: Tick(PPQ),
            },
            Note {
                pitch: Pitch::new(64),
                velocity: Velocity::MF,
                start: Tick(PPQ),
                duration: Tick(PPQ),
            },
        ];
        let pattern = Pattern::new(notes, Tick(PPQ * 2)).unwrap();
        s.dispatch(Command::InsertClip {
            track: s.tracks[0].id,
            name: "melody".into(),
            pattern,
            at: Tick::ZERO,
        })
        .unwrap();
        s
    }

    #[test]
    fn render_is_deterministic_and_audible() {
        let state = demo_project();
        let opts = RenderOptions::default();
        let a = render_project(&state, &opts).unwrap();
        let b = render_project(&state, &opts).unwrap();
        assert_eq!(a, b, "same project must render bit-identically");
        assert!(a.peak() > 0.05, "render must not be silent");
        assert!(
            a.peak() <= opts.peak_ceiling + 1e-6,
            "peak ceiling enforced"
        );
        // 2 quarters at 120 BPM = 1s of held notes, plus release + tail.
        assert!(a.frames() >= 48_000);
    }

    #[test]
    fn empty_projects_are_rejected() {
        let s = ProjectState::new(ProjectId(1), "Empty");
        assert!(matches!(
            render_project(&s, &RenderOptions::default()),
            Err(RenderError::EmptyProject)
        ));
    }

    #[test]
    fn wav_writing_round_trips_header() {
        let state = demo_project();
        let path = std::env::temp_dir().join(format!("musicos-render-{}.wav", std::process::id()));
        let report = render_to_wav(&state, &RenderOptions::default(), &path).unwrap();
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, 48_000);
        assert_eq!(reader.duration() as usize, report.frames);
        std::fs::remove_file(&path).unwrap();
    }
}
