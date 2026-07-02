//! MusicOS command-line client (`music`).
//!
//! Phase 1 milestone 1: real symbolic operations on Standard MIDI Files.
//! Commands will migrate onto the tool registry (`docs/02` §4) when it lands,
//! so every command here is already shaped like a tool: typed input, typed
//! output, `--json` mode, machine-readable errors (FR-CLI2).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use musicos_core_types::{Seed, Tick};
use musicos_midi::{export_smf, import_smf, SmfSong};

#[derive(Parser)]
#[command(
    name = "music",
    version,
    about = "MusicOS — autonomous music production"
)]
struct Cli {
    /// Emit machine-readable JSON on stdout.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show summary information about a MIDI file.
    Info {
        /// Input .mid file.
        input: PathBuf,
    },
    /// Transpose all tracks of a MIDI file by a number of semitones.
    Transpose {
        /// Input .mid file.
        input: PathBuf,
        /// Output .mid file.
        #[arg(short, long)]
        output: PathBuf,
        /// Semitones to transpose by (negative = down).
        #[arg(short, long, allow_hyphen_values = true)]
        semitones: i8,
    },
    /// Quantize note starts to a grid.
    Quantize {
        /// Input .mid file.
        input: PathBuf,
        /// Output .mid file.
        #[arg(short, long)]
        output: PathBuf,
        /// Grid size in ticks (960 = quarter note, 240 = sixteenth).
        #[arg(short, long, default_value_t = 240)]
        grid: i64,
        /// Quantize strength in percent (100 = full snap).
        #[arg(long, default_value_t = 100)]
        strength: u8,
    },
    /// Humanize timing and velocity with a deterministic seed.
    Humanize {
        /// Input .mid file.
        input: PathBuf,
        /// Output .mid file.
        #[arg(short, long)]
        output: PathBuf,
        /// Random seed — the same seed always produces the same result.
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Maximum timing jitter in ticks.
        #[arg(long, default_value_t = 20)]
        timing: i64,
        /// Maximum velocity jitter in steps.
        #[arg(long, default_value_t = 8)]
        velocity: u8,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(report) => {
            if cli.json {
                println!("{report}");
            } else {
                let v: serde_json::Value =
                    serde_json::from_str(&report).expect("report is valid JSON");
                if let Some(s) = v.get("summary").and_then(|s| s.as_str()) {
                    println!("{s}");
                }
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            if cli.json {
                eprintln!("{}", serde_json::json!({ "error": err.to_string() }));
            } else {
                eprintln!("error: {err}");
            }
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> anyhow::Result<String> {
    match &cli.command {
        Command::Info { input } => {
            let song = load(input)?;
            let notes: usize = song.tracks.iter().map(|(_, p)| p.notes().len()).sum();
            let end = song
                .tracks
                .iter()
                .map(|(_, p)| p.length())
                .max()
                .unwrap_or(Tick::ZERO);
            let micros = song.tempo_map.tick_to_micros(end);
            #[allow(clippy::cast_precision_loss)] // display only; songs are << 2^52 µs
            let seconds = micros as f64 / 1e6;
            Ok(serde_json::json!({
                "tracks": song.tracks.iter().map(|(name, p)| serde_json::json!({
                    "name": name, "notes": p.notes().len(), "length_ticks": p.length().0,
                })).collect::<Vec<_>>(),
                "tempo_bpm": song.tempo_map.tempo_at(Tick::ZERO).bpm(),
                "tempo_changes": song.tempo_map.entries().len(),
                "duration_seconds": seconds,
                "summary": format!(
                    "{} track(s), {notes} notes, {seconds:.1}s at {:.1} BPM",
                    song.tracks.len(),
                    song.tempo_map.tempo_at(Tick::ZERO).bpm(),
                ),
            })
            .to_string())
        }
        Command::Transpose {
            input,
            output,
            semitones,
        } => transform(input, output, "transposed", |p| p.transposed(*semitones)),
        Command::Quantize {
            input,
            output,
            grid,
            strength,
        } => {
            anyhow::ensure!(*grid > 0, "grid must be positive");
            transform(input, output, "quantized", |p| {
                p.quantized(Tick(*grid), *strength)
            })
        }
        Command::Humanize {
            input,
            output,
            seed,
            timing,
            velocity,
        } => transform(input, output, "humanized", |p| {
            p.humanized(Seed(*seed), Tick(*timing), *velocity)
        }),
    }
}

fn transform(
    input: &PathBuf,
    output: &PathBuf,
    verb: &str,
    f: impl Fn(&musicos_music_core::Pattern) -> musicos_music_core::Pattern,
) -> anyhow::Result<String> {
    let mut song = load(input)?;
    let mut notes = 0usize;
    for (_, pattern) in &mut song.tracks {
        *pattern = f(pattern);
        notes += pattern.notes().len();
    }
    std::fs::write(output, export_smf(&song))?;
    Ok(serde_json::json!({
        "output": output.display().to_string(),
        "tracks": song.tracks.len(),
        "notes": notes,
        "summary": format!("{verb} {notes} notes across {} track(s) -> {}",
            song.tracks.len(), output.display()),
    })
    .to_string())
}

fn load(path: &PathBuf) -> anyhow::Result<SmfSong> {
    let bytes =
        std::fs::read(path).map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
    Ok(import_smf(&bytes)?)
}
