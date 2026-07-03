//! Offline render pipeline and audio export.
//!
//! Phase 2 milestone 2: rendering runs on the compiled audio graph
//! (`docs/04` §3). The project is translated into nodes — one `TrackSource`
//! per MIDI track, a strip node applying the track's [`ChannelStrip`]
//! (gain/pan/mute), and a master sum sink — compiled to a schedule with a
//! liveness-assigned buffer pool, and executed block by block. Renders remain
//! deterministic per platform (NFR-4).
//!
//! `TrackSource` pre-renders its track's notes offline and streams blocks;
//! the true streaming voice manager replaces it with the real-time engine
//! (`docs/04` §5). Latency compensation waits for latency-reporting nodes.

use std::path::Path;

use musicos_audio_graph::{CompiledGraph, Graph, Node, StereoBlock};
use musicos_core_types::Tick;
use musicos_dsp::{
    db_to_gain, pan_gains, BiquadMode, BiquadStereo, Compressor, Reverb, StereoBuffer, StereoDelay,
};
use musicos_instruments::SimpleSynth;
use musicos_project_model::{ChannelStrip, Device, EqMode, ProjectState, TrackKind};

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

/// A source node streaming a pre-rendered mono track.
struct TrackSource {
    mono: Vec<f32>,
}

impl Node for TrackSource {
    fn process(&mut self, frame_offset: usize, _: &[&StereoBlock], out: &mut StereoBlock) {
        for (i, (l, r)) in out.left.iter_mut().zip(out.right.iter_mut()).enumerate() {
            let s = self.mono.get(frame_offset + i).copied().unwrap_or(0.0);
            *l = s;
            *r = s;
        }
    }
}

/// An insert-effect node wrapping a DSP processor.
enum InsertNode {
    Passthrough,
    Eq(BiquadStereo),
    Compressor(Compressor),
    Delay(StereoDelay),
    Reverb(Reverb),
}

impl InsertNode {
    fn build(device: Device, sample_rate: u32) -> InsertNode {
        match device {
            Device::Eq {
                mode,
                freq_hz,
                q,
                gain_db,
            } => {
                let mode = match mode {
                    EqMode::LowPass => BiquadMode::LowPass,
                    EqMode::HighPass => BiquadMode::HighPass,
                    // Peak, and any future mode: neutral at gain 0 (docs/08 §5).
                    _ => BiquadMode::Peak,
                };
                InsertNode::Eq(BiquadStereo::new(mode, sample_rate, freq_hz, q, gain_db))
            }
            Device::Compressor {
                threshold_db,
                ratio,
                attack_ms,
                release_ms,
                makeup_db,
            } => InsertNode::Compressor(Compressor::new(
                sample_rate,
                threshold_db,
                ratio,
                attack_ms,
                release_ms,
                makeup_db,
            )),
            Device::Delay {
                time_ms,
                feedback,
                mix,
            } => InsertNode::Delay(StereoDelay::new(sample_rate, time_ms, feedback, mix)),
            Device::Reverb { room, damping, mix } => {
                InsertNode::Reverb(Reverb::new(sample_rate, room, damping, mix))
            }
            // Unknown device kinds from newer bundles: passthrough, never
            // refuse to render (forward tolerance, docs/08 §5).
            _ => InsertNode::Passthrough,
        }
    }
}

impl Node for InsertNode {
    fn process(&mut self, _: usize, inputs: &[&StereoBlock], out: &mut StereoBlock) {
        out.left.copy_from_slice(&inputs[0].left);
        out.right.copy_from_slice(&inputs[0].right);
        match self {
            InsertNode::Passthrough => {}
            InsertNode::Eq(p) => p.process(&mut out.left, &mut out.right),
            InsertNode::Compressor(p) => p.process(&mut out.left, &mut out.right),
            InsertNode::Delay(p) => p.process(&mut out.left, &mut out.right),
            InsertNode::Reverb(p) => p.process(&mut out.left, &mut out.right),
        }
    }
}

/// Applies a channel strip: mute, dB gain, equal-power pan.
struct StripNode {
    left_gain: f32,
    right_gain: f32,
}

impl StripNode {
    fn new(strip: ChannelStrip) -> StripNode {
        if strip.muted {
            return StripNode {
                left_gain: 0.0,
                right_gain: 0.0,
            };
        }
        let gain = db_to_gain(strip.gain_db);
        let (l, r) = pan_gains(strip.pan);
        StripNode {
            left_gain: gain * l,
            right_gain: gain * r,
        }
    }
}

impl Node for StripNode {
    fn process(&mut self, _: usize, inputs: &[&StereoBlock], out: &mut StereoBlock) {
        let input = inputs[0];
        for (o, i) in out.left.iter_mut().zip(&input.left) {
            *o = i * self.left_gain;
        }
        for (o, i) in out.right.iter_mut().zip(&input.right) {
            *o = i * self.right_gain;
        }
    }
}

/// Sums every input (the master bus sink).
struct MasterSum;

impl Node for MasterSum {
    fn process(&mut self, _: usize, inputs: &[&StereoBlock], out: &mut StereoBlock) {
        out.clear();
        for input in inputs {
            for (o, i) in out.left.iter_mut().zip(&input.left) {
                *o += i;
            }
            for (o, i) in out.right.iter_mut().zip(&input.right) {
                *o += i;
            }
        }
    }
}

/// Compiles a project into an executable graph plus the total frame count.
///
/// # Errors
/// Returns [`RenderError::EmptyProject`] if no MIDI clip contains notes, or
/// propagates graph compilation failures (impossible for this topology).
pub fn compile_project(
    state: &ProjectState,
    opts: &RenderOptions,
) -> Result<(CompiledGraph, usize), RenderError> {
    let synth = SimpleSynth::default();
    let sr = opts.sample_rate;

    // Pre-render each MIDI track to mono and find the total length.
    let mut track_audio: Vec<(ChannelStrip, Vec<Device>, Vec<f32>)> = Vec::new();
    let mut last_end = 0usize;
    for track in state.tracks.iter().filter(|t| t.kind == TrackKind::Midi) {
        let mut end_of_track = 0usize;
        for placement in &track.placements {
            let clip = &state.clips[&placement.clip];
            for note in clip.pattern.notes() {
                let end =
                    sample_at(state, placement.at + note.end(), sr) + synth.rendered_len(0, sr);
                end_of_track = end_of_track.max(end);
            }
        }
        if end_of_track == 0 {
            continue;
        }
        let mut mono = vec![0.0f32; end_of_track];
        for placement in &track.placements {
            let clip = &state.clips[&placement.clip];
            for note in clip.pattern.notes() {
                let start = sample_at(state, placement.at + note.start, sr);
                let end = sample_at(state, placement.at + note.end(), sr);
                let held = end.saturating_sub(start).max(1);
                let gain = f32::from(note.velocity.get()) / 127.0;
                for (i, s) in synth
                    .render_note(note.pitch, gain, held, sr)
                    .iter()
                    .enumerate()
                {
                    if let Some(slot) = mono.get_mut(start + i) {
                        *slot += s;
                    }
                }
            }
        }
        last_end = last_end.max(end_of_track);
        track_audio.push((track.mix, track.inserts.clone(), mono));
    }
    if track_audio.is_empty() {
        return Err(RenderError::EmptyProject);
    }

    let tail = (opts.tail_seconds * sr as f32).ceil() as usize;
    let total = last_end + tail;

    let mut graph = Graph::new();
    let master = graph.add(Box::new(MasterSum));
    for (strip, inserts, mono) in track_audio {
        let source = graph.add(Box::new(TrackSource { mono }));
        // Chain: source -> inserts (in order) -> strip -> master.
        let mut upstream = source;
        for device in inserts {
            let node = graph.add(Box::new(InsertNode::build(device, sr)));
            graph.connect(upstream, node).map_err(RenderError::Graph)?;
            upstream = node;
        }
        let strip_node = graph.add(Box::new(StripNode::new(strip)));
        graph
            .connect(upstream, strip_node)
            .map_err(RenderError::Graph)?;
        graph
            .connect(strip_node, master)
            .map_err(RenderError::Graph)?;
    }
    let compiled = graph.compile(master).map_err(RenderError::Graph)?;
    Ok((compiled, total))
}

/// Renders a project to a stereo buffer via the compiled graph.
///
/// # Errors
/// See [`compile_project`].
pub fn render_project(
    state: &ProjectState,
    opts: &RenderOptions,
) -> Result<StereoBuffer, RenderError> {
    let (mut compiled, total) = compile_project(state, opts)?;
    let (left, right) = compiled.render(total);
    let mut master = StereoBuffer { left, right };
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
        writer.write_sample((s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)?;
    }
    writer.finalize()?;
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
    /// Graph construction failed (unexpected for the fixed topology).
    #[error("graph: {0}")]
    Graph(musicos_audio_graph::GraphError),
    /// WAV encoding failure.
    #[error("wav: {0}")]
    Wav(#[from] hound::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicos_core_types::{Pitch, ProjectId, Tick, TrackId, Velocity, PPQ};
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
        assert!(a.frames() >= 48_000);
    }

    #[test]
    fn mute_silences_and_gain_scales() {
        let mut state = demo_project();
        let track = TrackId(0);
        let opts = RenderOptions::default();
        let loud = render_project(&state, &opts).unwrap();

        state
            .dispatch(Command::SetTrackGain {
                track,
                gain_db: -20.0,
            })
            .unwrap();
        let quiet = render_project(&state, &opts).unwrap();
        assert!(
            quiet.peak() < loud.peak() * 0.2,
            "-20 dB must be ~10x quieter"
        );

        state
            .dispatch(Command::SetTrackMute { track, muted: true })
            .unwrap();
        assert!(matches!(
            render_project(&state, &opts),
            Ok(b) if b.peak() == 0.0
        ));
    }

    #[test]
    fn pan_hard_left_empties_right_channel() {
        let mut state = demo_project();
        state
            .dispatch(Command::SetTrackPan {
                track: TrackId(0),
                pan: -1.0,
            })
            .unwrap();
        let out = render_project(&state, &RenderOptions::default()).unwrap();
        let right_peak = out.right.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let left_peak = out.left.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            right_peak < 1e-6,
            "hard-left pan must silence the right channel"
        );
        assert!(left_peak > 0.05);
    }

    #[test]
    fn a_delay_insert_audibly_extends_the_tail() {
        let mut state = demo_project();
        let opts = RenderOptions {
            tail_seconds: 2.0,
            ..RenderOptions::default()
        };
        let dry = render_project(&state, &opts).unwrap();
        state
            .dispatch(Command::AddDevice {
                track: TrackId(0),
                device: musicos_project_model::Device::Delay {
                    time_ms: 500.0,
                    feedback: 0.5,
                    mix: 0.5,
                },
            })
            .unwrap();
        let wet = render_project(&state, &opts).unwrap();
        // Measure energy in the final second (pure tail region).
        let window = 48_000usize;
        let tail_energy = |b: &musicos_dsp::StereoBuffer| {
            b.left[b.frames() - window..]
                .iter()
                .map(|s| s * s)
                .sum::<f32>()
        };
        assert!(
            tail_energy(&wet) > tail_energy(&dry) * 4.0 + 1e-6,
            "delay feedback must ring into the tail"
        );
        // Determinism holds with DSP in the chain.
        assert_eq!(wet, render_project(&state, &opts).unwrap());
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
        assert_eq!(reader.duration() as usize, report.frames);
        std::fs::remove_file(&path).unwrap();
    }
}
