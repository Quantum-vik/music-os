//! Real-time audio runtime: transport, graph execution, and voice management.
//!
//! Phase 4 engine (`docs/04` §2/§5): playback is **streaming** — sources
//! synthesize live per block with voice-managed polyphony (no pre-render),
//! the feeder thread runs the graph slightly ahead and fills a lock-free
//! SPSC ring, and the CPAL callback only pops — wait-free and
//! allocation-free on the device thread (ADR-0006, ADR-0015). The feeder
//! supports **graph swap at block boundaries** ([`SwapSlot`]) and
//! sample-accurate **seek** (start-frame transport). Remaining Phase 4 work:
//! live MIDI input and RT-thread graph execution proper (moving the graph
//! off the feeder onto the callback with a command ring).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use musicos_project_model::ProjectState;
use musicos_render::{compile_project_streaming, RenderOptions};

/// Ring capacity in frames (~0.5 s at 48 kHz — deep enough to ride out
/// feeder-thread scheduling hiccups).
const RING_FRAMES: usize = 24_000;

/// Playback progress: `(frames_played, total_frames)`.
pub type Progress = (u64, u64);

/// A slot for swapping the running graph at a block boundary (edit-during-
/// playback seam, docs/04 §2). The feeder checks it between blocks; the old
/// graph is dropped on the feeder thread, never the device thread.
#[derive(Clone, Default)]
pub struct SwapSlot(Arc<std::sync::Mutex<Option<(musicos_audio_graph::CompiledGraph, usize)>>>);

impl SwapSlot {
    /// Creates an empty slot.
    pub fn new() -> SwapSlot {
        SwapSlot::default()
    }

    /// Installs a replacement graph (+ its total frame count); it takes
    /// effect at the feeder's next block boundary.
    pub fn install(&self, graph: musicos_audio_graph::CompiledGraph, total_frames: usize) {
        *self.0.lock().expect("swap slot lock") = Some((graph, total_frames));
    }

    fn take(&self) -> Option<(musicos_audio_graph::CompiledGraph, usize)> {
        self.0.lock().expect("swap slot lock").take()
    }
}

/// Runs the feeder loop: executes `graph` from `start_frame`, pushing
/// interleaved stereo into `producer`, honoring `swap` at block boundaries
/// and `stop`. Pure of any audio device — unit-testable.
fn run_feeder(
    mut graph: musicos_audio_graph::CompiledGraph,
    mut total_frames: usize,
    start_frame: usize,
    mut producer: rtrb::Producer<f32>,
    stop: &AtomicBool,
    swap: &SwapSlot,
) {
    let mut written = start_frame;
    'outer: while written < total_frames {
        if let Some((new_graph, new_total)) = swap.take() {
            graph = new_graph;
            total_frames = new_total;
            if written >= total_frames {
                break;
            }
        }
        let block = graph.process_block(written);
        let take = musicos_audio_graph::BLOCK.min(total_frames - written);
        for i in 0..take {
            loop {
                if stop.load(Ordering::Relaxed) {
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
}

/// Plays a project through the default output device, blocking until done.
///
/// `on_progress` is called from the control thread roughly four times per
/// second.
///
/// # Errors
/// Returns [`PlaybackError`] when no device is available, the stream cannot
/// be built, or the project has nothing to play.
pub fn play(state: &ProjectState, on_progress: impl FnMut(Progress)) -> Result<(), PlaybackError> {
    play_from(state, 0, on_progress)
}

/// Plays a project starting at a bar (4/4), blocking until done.
///
/// # Errors
/// Same as [`play`].
pub fn play_from(
    state: &ProjectState,
    start_bar: u64,
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
    let (graph, total_frames) = compile_project_streaming(state, &opts)
        .map_err(|e| PlaybackError::Compile(e.to_string()))?;
    let start_tick = musicos_core_types::Tick(
        i64::try_from(start_bar).unwrap_or(0) * musicos_core_types::PPQ * 4,
    );
    #[allow(clippy::cast_sign_loss)]
    let start_frame = usize::try_from(
        state
            .tempo_map
            .tick_to_samples(start_tick, sample_rate)
            .max(0),
    )
    .unwrap_or(0)
    .min(total_frames);
    let total = total_frames as u64;

    // SPSC ring: feeder pushes interleaved stereo, callback pops.
    let ring = rtrb::RingBuffer::<f32>::new(RING_FRAMES * 2);
    let (producer, mut consumer) = ring;
    let played = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));

    // Feeder thread: run the streaming graph ahead of the callback.
    let feeder_stop = Arc::clone(&stop);
    let swap = SwapSlot::new();
    let feeder_swap = swap.clone();
    let feeder = std::thread::spawn(move || {
        run_feeder(
            graph,
            total_frames,
            start_frame,
            producer,
            &feeder_stop,
            &feeder_swap,
        );
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
    let remaining = total - start_frame as u64;
    loop {
        let done = (start_frame as u64 + played.load(Ordering::Relaxed)).min(total);
        let _ = remaining;
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

    #[test]
    fn feeder_honors_graph_swap_and_seek() {
        use musicos_audio_graph::{Graph, Node, StereoBlock};

        struct Constant(f32);
        impl Node for Constant {
            fn process(&mut self, _: usize, _: &[&StereoBlock], out: &mut StereoBlock) {
                out.left.fill(self.0);
                out.right.fill(self.0);
            }
        }
        fn constant_graph(v: f32) -> musicos_audio_graph::CompiledGraph {
            let mut g = Graph::new();
            let n = g.add(Box::new(Constant(v)));
            g.compile(n).unwrap()
        }

        let (producer, mut consumer) = rtrb::RingBuffer::<f32>::new(1 << 20);
        let stop = AtomicBool::new(false);
        let swap = SwapSlot::new();
        // Install the replacement BEFORE running: first block boundary picks
        // it up, so all audio comes from the swapped graph.
        swap.install(constant_graph(0.5), 2048);
        run_feeder(constant_graph(0.0), 2048, 1024, producer, &stop, &swap);

        let mut samples = Vec::new();
        while let Ok(s) = consumer.pop() {
            samples.push(s);
        }
        // Seek honored: only (2048 - 1024) frames * 2 channels produced.
        assert_eq!(samples.len(), 1024 * 2);
        // Swap honored: output is the swapped graph's value.
        assert!(samples.iter().all(|s| (*s - 0.5).abs() < 1e-6));
    }

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
        let (mut graph, total) = compile_project_streaming(&s, &opts).unwrap();
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
