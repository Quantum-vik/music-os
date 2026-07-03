//! `music-link` — stream a MusicOS project as MIDI, tempo-synced to an
//! Ableton Link session (docs/12 "DAW bridges").
//!
//! Joins the local Link session (Ableton Live, FL Studio 2024+, Bitwig, any
//! Link app on the LAN), waits for the next quantum boundary, and sends the
//! project's notes through a MIDI port at whatever tempo the session runs —
//! including live tempo changes mid-song. Licensed GPL-2.0-or-later because
//! Ableton Link is; this binary is intentionally isolated from the
//! Apache/MIT MusicOS workspace.
//!
//! Usage: `music-link <project.musicos> [--port <name>] [--from-bar N]
//! [--quantum BEATS]`

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use musicos_midi_stream::{schedule_beats, BeatEvent};
use rusty_link::{AblLink, SessionState};

fn usage() -> ! {
    eprintln!(
        "usage: music-link <project.musicos> [--port <name>] [--from-bar N] [--quantum BEATS]"
    );
    std::process::exit(2);
}

struct Args {
    project: std::path::PathBuf,
    port: Option<String>,
    from_bar: u64,
    quantum: f64,
}

fn parse_args() -> Args {
    let mut args = Args {
        project: std::path::PathBuf::new(),
        port: None,
        from_bar: 0,
        quantum: 4.0,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--port" => args.port = Some(it.next().unwrap_or_else(|| usage())),
            "--from-bar" => {
                args.from_bar = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| usage());
            }
            "--quantum" => {
                args.quantum = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| usage());
            }
            "--help" | "-h" => usage(),
            other if args.project.as_os_str().is_empty() => {
                args.project = std::path::PathBuf::from(other);
            }
            _ => usage(),
        }
    }
    if args.project.as_os_str().is_empty() {
        usage();
    }
    args
}

fn main() {
    let args = parse_args();
    let state = musicos_storage::BundleStore::open(&args.project)
        .and_then(|s| s.load_state())
        .unwrap_or_else(|e| {
            eprintln!("music-link: {e}");
            std::process::exit(1);
        });
    let events: Vec<BeatEvent> = schedule_beats(&state, args.from_bar);
    if events.is_empty() {
        eprintln!("music-link: project has no notes to stream");
        std::process::exit(1);
    }

    // MIDI out: virtual port by default (macOS/Linux), named port otherwise.
    let midi = midir::MidiOutput::new("MusicOS Link").expect("midi output");
    let mut connection = match &args.port {
        Some(fragment) => {
            let port = midi
                .ports()
                .into_iter()
                .find(|p| {
                    midi.port_name(p)
                        .is_ok_and(|n| n.to_lowercase().contains(&fragment.to_lowercase()))
                })
                .unwrap_or_else(|| {
                    eprintln!("music-link: no MIDI output port matching '{fragment}'");
                    std::process::exit(1);
                });
            midi.connect(&port, "musicos-link").expect("midi connect")
        }
        None => {
            #[cfg(unix)]
            {
                use midir::os::unix::VirtualOutput as _;
                midi.create_virtual("MusicOS Link Out")
                    .expect("virtual port")
            }
            #[cfg(not(unix))]
            {
                eprintln!("music-link: pass --port <loopMIDI port> on Windows");
                std::process::exit(1);
            }
        }
    };

    let stop = Arc::new(AtomicBool::new(false));
    let handler_stop = Arc::clone(&stop);
    let _ = ctrlc::set_handler(move || handler_stop.store(true, Ordering::Relaxed));

    // Join the Link session and launch at the next quantum boundary.
    let link = AblLink::new(120.0);
    link.enable(true);
    let mut session = SessionState::new();
    link.capture_app_session_state(&mut session);
    let now = link.clock_micros();
    let launch_beat = {
        let beat = session.beat_at_time(now, args.quantum);
        (beat / args.quantum)
            .floor()
            .mul_add(args.quantum, args.quantum)
    };
    eprintln!(
        "music-link: joined session at {:.1} bpm ({} peer(s)); launching at beat {launch_beat:.1} \
         (quantum {}) — Ctrl-C to stop",
        session.tempo(),
        link.num_peers(),
        args.quantum
    );

    let total = events.len();
    let mut sent = 0usize;
    for event in &events {
        let target_beat = launch_beat + event.at_beats;
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            link.capture_app_session_state(&mut session);
            let target_time = session.time_at_beat(target_beat, args.quantum);
            let wait_micros = target_time - link.clock_micros();
            if wait_micros <= 0 {
                break;
            }
            #[allow(clippy::cast_sign_loss)]
            std::thread::sleep(Duration::from_micros((wait_micros as u64).min(5_000)));
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        connection.send(&event.bytes).expect("midi send");
        sent += 1;
        if sent % 16 == 0 || sent == total {
            eprint!("\r{sent}/{total} events @ {:.1} bpm", session.tempo());
        }
    }
    eprintln!();
    for channel in 0..16u8 {
        let _ = connection.send(&[0xB0 | channel, 123, 0]); // all-notes-off
    }
    connection.close();
    link.enable(false);
}
