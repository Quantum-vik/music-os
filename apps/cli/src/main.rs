//! MusicOS command-line client (`music`).
//!
//! Project commands dispatch through the canonical tool registry
//! (`docs/02` §4) — the same tools the MCP server publishes in Phase 3, so the
//! CLI and MCP surfaces cannot drift apart. File-level MIDI utilities live
//! under `music midi …`. Every command supports `--json` and exits non-zero
//! with a machine-readable error code on failure (FR-CLI2).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use musicos_ai_providers::{find_server_binary, AnthropicBackend, Provider, SubscriptionRunner};
use musicos_ai_runtime::{run_agent, AgentConfig};
use musicos_core_types::{ProjectId, Seed, Tick};
use musicos_midi::{export_smf, import_smf, SmfSong};
use musicos_project_model::ProjectState;
use musicos_storage::BundleStore;
use musicos_tools::{ProjectCtx, Registry};
use serde_json::{json, Value};

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
    /// Project bundle directory (defaults to the single *.musicos in cwd).
    #[arg(short = 'P', long, global = true)]
    project: Option<PathBuf>,
    /// Increase log verbosity (-v info, -vv debug, -vvv trace).
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    verbose: u8,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new project bundle.
    Init {
        /// Project name; the bundle is created at `<name>.musicos` unless --dir is given.
        name: String,
        /// Explicit bundle directory to create.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Show a summary of the project.
    Info,
    /// Track operations.
    #[command(subcommand)]
    Track(TrackCmd),
    /// Import a .mid file: each MIDI track becomes a project track with one clip.
    Import {
        /// Input .mid file.
        input: PathBuf,
        /// Timeline position in ticks (960 PPQ).
        #[arg(long, default_value_t = 0)]
        at: i64,
    },
    /// Set the project tempo.
    Tempo {
        /// Beats per minute.
        bpm: f64,
        /// Timeline position in ticks.
        #[arg(long, default_value_t = 0)]
        at: i64,
    },
    /// Play the project through the default audio output.
    Play {
        /// Start playback at this bar (4/4, 0-based).
        #[arg(long, default_value_t = 0)]
        from_bar: u64,
    },
    /// Undo the most recent project transaction.
    Undo,
    /// Set track mix parameters (gain/pan/mute).
    Mix {
        /// Track id (see `music info`).
        track: u64,
        /// Gain in dB.
        #[arg(long, allow_hyphen_values = true)]
        gain: Option<f32>,
        /// Pan, -1.0 (left) to 1.0 (right).
        #[arg(long, allow_hyphen_values = true)]
        pan: Option<f32>,
        /// Mute (true/false).
        #[arg(long)]
        mute: Option<bool>,
    },
    /// Render the project to a WAV file with the built-in synthesizer.
    Render {
        /// Output .wav path.
        #[arg(short, long, default_value = "render.wav")]
        output: PathBuf,
        /// Sample rate in Hz. Defaults to config `render.sample_rate`.
        #[arg(long)]
        rate: Option<u32>,
    },
    /// Run an AI production agent over the project (subscription or API).
    Ai {
        /// What you want done, in plain language.
        brief: String,
        /// Provider: subscription (Claude Code, no API keys) | api | auto.
        /// Defaults to config `ai.provider`, else auto.
        #[arg(long)]
        provider: Option<String>,
        /// Model id for api mode. Defaults to config `ai.model`.
        #[arg(long)]
        model: Option<String>,
        /// Maximum model round-trips (api mode). Defaults to config `ai.max_turns`.
        #[arg(long)]
        max_turns: Option<u32>,
    },
    /// Call any registered tool by name with a JSON input (full CLI/MCP parity).
    Call {
        /// Tool name (see `music tools`).
        tool: String,
        /// JSON input object. Default: {}.
        #[arg(default_value = "{}")]
        input: String,
    },
    /// List native plugins and installed CLAP bundles.
    Plugins {
        /// Load a specific .clap file and list the plugins inside it.
        #[arg(long)]
        probe: Option<std::path::PathBuf>,
    },
    /// List every registered tool and its JSON input schema.
    Tools,
    /// File-level MIDI utilities (no project needed).
    #[command(subcommand)]
    Midi(MidiCmd),
}

#[derive(Subcommand)]
enum TrackCmd {
    /// Add a track.
    Add {
        /// Track name.
        name: String,
        /// Track kind: midi | audio | bus.
        #[arg(long, default_value = "midi")]
        kind: String,
    },
    /// Remove a track by id.
    Remove {
        /// Track id (see `music info`).
        id: u64,
    },
}

#[derive(Subcommand)]
enum MidiCmd {
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
        /// Semitones (negative = down).
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
        /// Strength percent (100 = full snap).
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
        /// Random seed — same seed, same result.
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
    musicos_telemetry::init(cli.verbose);
    let loaded = musicos_config::Config::load(cli.project.as_deref());
    if cli.verbose > 0 {
        for warning in &loaded.warnings {
            eprintln!("[musicos] config: {warning}");
        }
    }
    match run(&cli, &loaded.config) {
        Ok(report) => {
            if cli.json {
                println!("{report}");
            } else if let Some(s) = report.get("summary").and_then(Value::as_str) {
                println!("{s}");
            } else {
                println!("{report:#}");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            let (code, message) = match err.downcast_ref::<musicos_tools::ToolError>() {
                Some(te) => (te.code, te.message.clone()),
                None => ("E_CLI", err.to_string()),
            };
            if cli.json {
                eprintln!(
                    "{}",
                    json!({ "error": { "code": code, "message": message } })
                );
            } else {
                eprintln!("error [{code}]: {message}");
            }
            ExitCode::FAILURE
        }
    }
}

#[allow(clippy::too_many_lines)] // one arm per subcommand; commands migrate onto the registry over time
fn run(cli: &Cli, config: &musicos_config::Config) -> anyhow::Result<Value> {
    match &cli.command {
        Command::Init { name, dir } => {
            let dir = dir
                .clone()
                .unwrap_or_else(|| PathBuf::from(format!("{name}.musicos")));
            let id = ProjectId(rand_id());
            BundleStore::create(&dir, &ProjectState::new(id, name))?;
            Ok(json!({
                "path": dir.display().to_string(),
                "summary": format!("created project '{name}' at {}", dir.display()),
            }))
        }
        Command::Info => call_tool(cli, "get_project_summary", json!({})),
        Command::Track(TrackCmd::Add { name, kind }) => {
            call_tool(cli, "add_track", json!({ "name": name, "kind": kind }))
        }
        Command::Track(TrackCmd::Remove { id }) => {
            call_tool(cli, "remove_track", json!({ "track_id": id }))
        }
        Command::Import { input, at } => call_tool(
            cli,
            "import_midi",
            json!({ "path": input.display().to_string(), "at": at }),
        ),
        Command::Tempo { bpm, at } => call_tool(cli, "set_tempo", json!({ "bpm": bpm, "at": at })),
        Command::Play { from_bar } => {
            let path = resolve_project(cli.project.as_deref())?;
            let ctx = ProjectCtx::open(&path, "user:cli")?;
            let state = ctx.state().clone();
            eprintln!("[musicos] playing '{}' — ctrl-c to stop", state.meta.name);
            let mut last_sec = u64::MAX;
            musicos_audio_engine::play_from(&state, *from_bar, |(done, total)| {
                let sec = done / 48_000;
                if sec != last_sec {
                    last_sec = sec;
                    eprint!("\r{:>4}s / {}s", sec, total / 48_000);
                }
            })?;
            eprintln!();
            Ok(json!({ "summary": "playback finished" }))
        }
        Command::Undo => call_tool(cli, "undo", json!({})),
        Command::Mix {
            track,
            gain,
            pan,
            mute,
        } => call_tool(
            cli,
            "set_track_mix",
            json!({ "track_id": track, "gain_db": gain, "pan": pan, "muted": mute }),
        ),
        Command::Render { output, rate } => call_tool(
            cli,
            "render_song",
            json!({
                "output": output.display().to_string(),
                "sample_rate": rate.unwrap_or(config.render.sample_rate),
            }),
        ),
        Command::Ai {
            brief,
            provider,
            model,
            max_turns,
        } => {
            let path = resolve_project(cli.project.as_deref())?;
            let provider = provider
                .clone()
                .or_else(|| config.ai.provider.clone())
                .unwrap_or_else(|| "auto".to_string());
            let model = model.clone().unwrap_or_else(|| config.ai.model.clone());
            let max_turns = max_turns.unwrap_or(config.ai.max_turns);
            match Provider::resolve(Some(provider.as_str()))? {
                Provider::Subscription => {
                    let runner = SubscriptionRunner {
                        server_bin: find_server_binary()?,
                        project: path,
                    };
                    eprintln!("[musicos] provider: subscription (Claude Code + MCP)");
                    runner.run(brief)?;
                    Ok(json!({ "summary": "agent run finished (subscription mode)" }))
                }
                Provider::Api => {
                    let backend = AnthropicBackend::from_env()?;
                    let mut ctx = ProjectCtx::open(&path, "agent:api")?;
                    let agent_config = AgentConfig {
                        model,
                        max_turns,
                        ..AgentConfig::default()
                    };
                    eprintln!("[musicos] provider: api (model {})", agent_config.model);
                    let outcome =
                        run_agent(&backend, &Registry::new(), &mut ctx, &agent_config, brief)?;
                    Ok(json!({
                        "reply": outcome.reply,
                        "turns": outcome.turns,
                        "tool_calls": outcome.tool_calls,
                        "budget_exhausted": outcome.budget_exhausted,
                        "summary": outcome.reply,
                    }))
                }
            }
        }
        Command::Call { tool, input } => {
            let parsed: Value = serde_json::from_str(input)
                .map_err(|e| anyhow::anyhow!("input is not valid JSON: {e}"))?;
            call_tool(cli, tool, parsed)
        }
        Command::Plugins { probe } => {
            if let Some(path) = probe {
                // SAFETY: probing runs the library's entry code; the user
                // explicitly named this file on the command line.
                let library = unsafe { musicos_plugin_host::clap_host::ClapLibrary::load(path) }?;
                let plugins: Vec<Value> = library
                    .plugins()?
                    .iter()
                    .map(|p| {
                        json!({
                            "id": p.id, "name": p.name,
                            "vendor": p.vendor, "version": p.version,
                        })
                    })
                    .collect();
                return Ok(json!({
                    "path": path.display().to_string(),
                    "plugins": plugins,
                    "summary": format!("{} plugin(s) in {}", plugins.len(), path.display()),
                }));
            }
            let registry = musicos_plugin_host::HostRegistry::with_builtins();
            let native: Vec<Value> = registry
                .descriptors()
                .iter()
                .map(|d| {
                    json!({
                        "id": d.id, "name": d.name, "vendor": d.vendor,
                        "version": d.version, "kind": format!("{:?}", d.kind),
                    })
                })
                .collect();
            let clap: Vec<String> = musicos_plugin_host::discover_clap()
                .iter()
                .map(|c| c.path.display().to_string())
                .collect();
            Ok(json!({
                "native": native,
                "clap": clap,
                "summary": format!(
                    "{} native plugin(s), {} CLAP bundle(s) installed (probe with --probe <path>)",
                    native.len(),
                    clap.len()
                ),
            }))
        }
        Command::Tools => {
            let specs: Vec<Value> = Registry::new()
                .specs()
                .iter()
                .map(|s| {
                    json!({
                        "name": s.name, "description": s.description, "params": s.params_schema,
                    })
                })
                .collect();
            let names: Vec<&str> = Registry::new().specs().iter().map(|s| s.name).collect();
            Ok(json!({
                "tools": specs,
                "summary": format!("{} tools: {}", names.len(), names.join(", ")),
            }))
        }
        Command::Midi(cmd) => run_midi(cmd),
    }
}

fn call_tool(cli: &Cli, name: &str, input: Value) -> anyhow::Result<Value> {
    let path = resolve_project(cli.project.as_deref())?;
    let mut ctx = ProjectCtx::open(&path, "user:cli")?;
    Ok(Registry::new().call(name, &mut ctx, input)?)
}

/// Uses `--project` if given, otherwise the single `*.musicos` directory in cwd.
fn resolve_project(flag: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(p) = flag {
        return Ok(p.to_path_buf());
    }
    let bundles: Vec<PathBuf> = std::fs::read_dir(".")?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.extension().is_some_and(|e| e == "musicos"))
        .collect();
    match bundles.as_slice() {
        [one] => Ok(one.clone()),
        [] => anyhow::bail!("no .musicos project here — run `music init <name>` or pass -P <dir>"),
        _ => anyhow::bail!("multiple .musicos projects here — pass -P <dir>"),
    }
}

/// Non-cryptographic id from system entropy (UUID backing lands in Phase 1 M3).
fn rand_id() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

fn run_midi(cmd: &MidiCmd) -> anyhow::Result<Value> {
    match cmd {
        MidiCmd::Info { input } => {
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
            Ok(json!({
                "tracks": song.tracks.iter().map(|(name, p)| json!({
                    "name": name, "notes": p.notes().len(), "length_ticks": p.length().0,
                })).collect::<Vec<_>>(),
                "tempo_bpm": song.tempo_map.tempo_at(Tick::ZERO).bpm(),
                "duration_seconds": seconds,
                "summary": format!(
                    "{} track(s), {notes} notes, {seconds:.1}s at {:.1} BPM",
                    song.tracks.len(),
                    song.tempo_map.tempo_at(Tick::ZERO).bpm(),
                ),
            }))
        }
        MidiCmd::Transpose {
            input,
            output,
            semitones,
        } => transform(input, output, "transposed", |p| p.transposed(*semitones)),
        MidiCmd::Quantize {
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
        MidiCmd::Humanize {
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
    input: &Path,
    output: &Path,
    verb: &str,
    f: impl Fn(&musicos_music_core::Pattern) -> musicos_music_core::Pattern,
) -> anyhow::Result<Value> {
    let mut song = load(input)?;
    let mut notes = 0usize;
    for (_, pattern) in &mut song.tracks {
        *pattern = f(pattern);
        notes += pattern.notes().len();
    }
    std::fs::write(output, export_smf(&song))?;
    Ok(json!({
        "output": output.display().to_string(),
        "tracks": song.tracks.len(),
        "notes": notes,
        "summary": format!("{verb} {notes} notes across {} track(s) -> {}",
            song.tracks.len(), output.display()),
    }))
}

fn load(path: &Path) -> anyhow::Result<SmfSong> {
    let bytes =
        std::fs::read(path).map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
    Ok(import_smf(&bytes)?)
}
