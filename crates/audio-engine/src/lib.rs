//! Real-time audio runtime: transport, graph execution, and voice management.
//!
//! v0 playback (`docs/04` §2, honest scope): a **feeder thread** executes the
//! compiled graph ahead of time and fills a lock-free SPSC ring; the CPAL
//! audio callback only pops samples — wait-free and allocation-free on the
//! device thread, Reaper-style anticipative rendering. The full streaming
//! engine (sample-accurate transport, live voice management, graph swap under
//! playback) is the Phase 4 milestone that replaces the feeder; the CPAL
//! adapter and ring topology built here carry over (ADR-0006, ADR-0015).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use musicos_project_model::ProjectState;
use musicos_render::{compile_project, RenderOptions};

/// Ring capacity in frames (~0.5 s at 48 kHz — deep enough to ride out
/// feeder-thread scheduling hiccups).
const RING_FRAMES: usize = 24_000;

/// Playback progress: `(frames_played, total_frames)`.
pub type Progress = (u64, u64);

/// Plays a project through the default output device, blocking until done.
///
/// `on_progress` is called from the control thread roughly four times per
/// second.
///
/// # Errors
/// Returns [`PlaybackError`] when no device is available, the stream cannot
/// be built, or the project has nothing to play.
pub fn play(
    state: &ProjectState,
    mut on_progress: impl FnMut(Progress),
) -> Result<(), PlaybackError> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(PlaybackError::NoDevice)?;
    let config = device
        .default_output_config()
        .map_err(|e| PlaybackError::Device(e.to_string()))?;
    if config.sample_format() != cpal::SampleFormat::F32 {
        return Err(PlaybackError::Device(format!(
            "unsupported sample format {:?} (only f32 outputs supported in v0)",
            config.sample_format()
        )));
    }
    let sample_rate = config.sample_rate().0;
    let channels = usize::from(config.channels());

    // Compile at the device rate so no resampling is needed.
    let opts = RenderOptions {
        sample_rate,
        ..RenderOptions::default()
    };
    let (mut graph, total_frames) =
        compile_project(state, &opts).map_err(|e| PlaybackError::Compile(e.to_string()))?;
    let total = total_frames as u64;

    // SPSC ring: feeder pushes interleaved stereo, callback pops.
    let ring = rtrb::RingBuffer::<f32>::new(RING_FRAMES * 2);
    let (mut producer, mut consumer) = ring;
    let played = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));

    // Feeder thread: run the graph ahead of the callback.
    let feeder_stop = Arc::clone(&stop);
    let feeder = std::thread::spawn(move || {
        let mut written = 0usize;
        'outer: while written < total_frames {
            let block = graph.process_block(written);
            let take = musicos_audio_graph::BLOCK.min(total_frames - written);
            for i in 0..take {
                // Busy-wait politely while the ring is full.
                loop {
                    if feeder_stop.load(Ordering::Relaxed) {
                        break 'outer;
                    }
                    if producer.slots() >= 2 {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                let _ = producer.push(block.left[i]);
                let _ = producer.push(block.right[i]);
            }
            written += take;
        }
    });

    // Device callback: pop only. Underruns emit silence.
    let cb_played = Arc::clone(&played);
    let stream = device
        .build_output_stream(
            &config.into(),
            move |data: &mut [f32], _| {
                let mut frames = 0u64;
                for frame in data.chunks_mut(channels) {
                    let l = consumer.pop().unwrap_or(0.0);
                    let r = consumer.pop().unwrap_or(l);
                    if channels == 1 {
                        frame[0] = (l + r) * 0.5;
                    } else {
                        frame[0] = l;
                        frame[1] = r;
                        for extra in frame.iter_mut().skip(2) {
                            *extra = 0.0;
                        }
                    }
                    frames += 1;
                }
                cb_played.fetch_add(frames, Ordering::Relaxed);
            },
            move |err| eprintln!("audio stream error: {err}"),
            None,
        )
        .map_err(|e| PlaybackError::Device(e.to_string()))?;
    stream
        .play()
        .map_err(|e| PlaybackError::Device(e.to_string()))?;

    // Control loop: report progress until everything audible has played.
    loop {
        let done = played.load(Ordering::Relaxed).min(total);
        on_progress((done, total));
        if done >= total {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    // Small grace period for the device's own buffer, then tear down.
    std::thread::sleep(Duration::from_millis(150));
    stop.store(true, Ordering::Relaxed);
    drop(stream);
    let _ = feeder.join();
    Ok(())
}

/// Errors from playback.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlaybackError {
    /// No default output device.
    #[error("no audio output device available")]
    NoDevice,
    /// Device/stream failure.
    #[error("audio device: {0}")]
    Device(String),
    /// The project could not be compiled for playback.
    #[error("compile: {0}")]
    Compile(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicos_core_types::{Pitch, ProjectId, Tick, Velocity, PPQ};
    use musicos_music_core::{Note, Pattern};
    use musicos_project_model::{Command, TrackKind};

    /// Playback compilation shares the render pipeline, so total length and
    /// determinism are covered there; here we pin the compile path used by
    /// `play` (device tests need hardware and stay out of CI).
    #[test]
    fn playback_compilation_matches_render_length() {
        let mut s = ProjectState::new(ProjectId(1), "Play");
        s.dispatch(Command::CreateTrack {
            name: "T".into(),
            kind: TrackKind::Midi,
        })
        .unwrap();
        let pattern = Pattern::new(
            vec![Note {
                pitch: Pitch::new(60),
                velocity: Velocity::MF,
                start: Tick::ZERO,
                duration: Tick(PPQ * 2),
            }],
            Tick(PPQ * 2),
        )
        .unwrap();
        s.dispatch(Command::InsertClip {
            track: s.tracks[0].id,
            name: "c".into(),
            pattern,
            at: Tick::ZERO,
        })
        .unwrap();
        let opts = RenderOptions::default();
        let (mut graph, total) = compile_project(&s, &opts).unwrap();
        // 2 quarters at 120 BPM = 1 s + synth tail + 0.5 s option tail.
        assert!(total > 48_000 && total < 48_000 * 3, "{total}");
        // The graph streams blocks from frame 0 without allocation surprises.
        let block = graph.process_block(0);
        assert!(
            block.left.iter().any(|s| s.abs() > 0.0),
            "audible from the start"
        );
    }
}
