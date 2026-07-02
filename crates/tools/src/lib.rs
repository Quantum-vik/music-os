//! Canonical tool registry mapping capabilities to CLI and MCP surfaces.
//!
//! Every capability is defined **once** as a [`Tool`] with a JSON-schema'd
//! input and structured output (`docs/02` §4). The CLI dispatches through this
//! registry today; the MCP server publishes the same specs in Phase 3, which is
//! how CLI/MCP parity is guaranteed structurally rather than by convention.
//!
//! v0 is synchronous; the async service runtime wraps it in Phase 3. Outputs
//! follow the `{ data..., summary }` convention (`docs/07` §3): programs act on
//! the data, language models act on the summary.

use musicos_composition::{
    chords_to_pattern, generate_bass, generate_chords, generate_drums, generate_melody, DrumStyle,
};
use musicos_core_types::{Seed, Tempo, Tick, TrackId};
use musicos_harmony::{parse_note_name, parse_scale_kind, Chord, Scale};
use musicos_music_core::Pattern;
use musicos_project_model::{Command, TrackKind};
use musicos_project_service::{ProjectSession, Transaction};
use musicos_storage::BundleStore;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

/// Machine-readable description of one tool.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    /// Stable `snake_case` name (CLI command and MCP tool name).
    pub name: &'static str,
    /// Description written for both humans and language models.
    pub description: &'static str,
    /// JSON Schema of the input object.
    pub params_schema: Value,
}

/// An open project plus everything a tool needs to act on it.
///
/// Mutating tools go through the private `commit` helper, which persists state and
/// appends the transaction to the bundle log — tools cannot forget to persist.
pub struct ProjectCtx {
    session: ProjectSession,
    store: BundleStore,
    actor: String,
}

impl ProjectCtx {
    /// Opens a project bundle for tool execution.
    ///
    /// # Errors
    /// Fails if the bundle cannot be read.
    pub fn open(path: &std::path::Path, actor: &str) -> Result<ProjectCtx, ToolError> {
        let store = BundleStore::open(path).map_err(ToolError::storage)?;
        let state = store.load_state().map_err(ToolError::storage)?;
        Ok(ProjectCtx {
            session: ProjectSession::from_state(state),
            store,
            actor: actor.to_string(),
        })
    }

    /// Read access to the current project state.
    pub fn state(&self) -> &musicos_project_model::ProjectState {
        self.session.state()
    }

    /// Dispatches a command and persists the result (state + log).
    fn commit(&mut self, command: Command) -> Result<Transaction, ToolError> {
        let txn = self
            .session
            .dispatch(&self.actor, command)
            .map_err(ToolError::domain)?;
        self.store
            .save_state(self.session.state())
            .map_err(ToolError::storage)?;
        self.store.append_log(&txn).map_err(ToolError::storage)?;
        Ok(txn)
    }
}

/// One capability, callable identically from CLI, MCP, and agents.
pub trait Tool {
    /// The tool's stable spec.
    fn spec(&self) -> ToolSpec;
    /// Executes with a JSON input matching [`ToolSpec::params_schema`].
    ///
    /// # Errors
    /// Returns [`ToolError`] with a stable machine code on failure.
    fn call(&self, ctx: &mut ProjectCtx, input: Value) -> Result<Value, ToolError>;
}

/// Structured tool failure with a stable machine-readable code (FR-CLI2).
#[derive(Debug, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ToolError {
    /// Stable error code (`E_INVALID_INPUT`, `E_DOMAIN`, `E_STORAGE`, …).
    pub code: &'static str,
    /// Human-readable detail.
    pub message: String,
}

impl ToolError {
    fn invalid(err: impl std::fmt::Display) -> ToolError {
        ToolError {
            code: "E_INVALID_INPUT",
            message: err.to_string(),
        }
    }
    fn domain(err: impl std::fmt::Display) -> ToolError {
        ToolError {
            code: "E_DOMAIN",
            message: err.to_string(),
        }
    }
    fn storage(err: impl std::fmt::Display) -> ToolError {
        ToolError {
            code: "E_STORAGE",
            message: err.to_string(),
        }
    }
}

fn parse<T: for<'de> Deserialize<'de>>(input: Value) -> Result<T, ToolError> {
    serde_json::from_value(input).map_err(ToolError::invalid)
}

fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema serializes")
}

// --- get_project_summary ------------------------------------------------------

/// Input for `get_project_summary`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SummaryInput {}

struct GetProjectSummary;

impl Tool for GetProjectSummary {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "get_project_summary",
            description: "Compact digest of the open project: name, tempo, tracks, \
                          clips. Cheap; prefer this over reading raw project files.",
            params_schema: schema::<SummaryInput>(),
        }
    }

    fn call(&self, ctx: &mut ProjectCtx, input: Value) -> Result<Value, ToolError> {
        let SummaryInput {} = parse(input)?;
        let s = ctx.state();
        let tracks: Vec<Value> = s
            .tracks
            .iter()
            .map(|t| {
                json!({
                    "id": t.id.0, "name": t.name, "kind": format!("{:?}", t.kind),
                    "clips": t.placements.len(),
                })
            })
            .collect();
        let bpm = s.tempo_map.tempo_at(Tick::ZERO).bpm();
        Ok(json!({
            "name": s.meta.name,
            "format_version": s.meta.format_version,
            "tempo_bpm": bpm,
            "tracks": tracks,
            "clip_count": s.clips.len(),
            "summary": format!(
                "'{}': {} track(s), {} clip(s), {:.1} BPM",
                s.meta.name, s.tracks.len(), s.clips.len(), bpm
            ),
        }))
    }
}

// --- add_track ----------------------------------------------------------------

/// Input for `add_track`.
#[derive(Debug, Deserialize, JsonSchema)]
struct AddTrackInput {
    /// Track display name.
    name: String,
    /// Track kind: "midi", "audio", or "bus". Defaults to "midi".
    #[serde(default)]
    kind: Option<String>,
}

struct AddTrack;

impl Tool for AddTrack {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "add_track",
            description: "Add a track to the project (kind: midi|audio|bus).",
            params_schema: schema::<AddTrackInput>(),
        }
    }

    fn call(&self, ctx: &mut ProjectCtx, input: Value) -> Result<Value, ToolError> {
        let input: AddTrackInput = parse(input)?;
        let kind = match input.kind.as_deref().unwrap_or("midi") {
            "midi" => TrackKind::Midi,
            "audio" => TrackKind::Audio,
            "bus" => TrackKind::Bus,
            other => return Err(ToolError::invalid(format!("unknown track kind '{other}'"))),
        };
        let txn = ctx.commit(Command::CreateTrack {
            name: input.name,
            kind,
        })?;
        let track = ctx.state().tracks.last().expect("track just created");
        let _ = txn;
        Ok(json!({
            "track_id": track.id.0,
            "summary": format!("added {:?} track '{}' (id {})", kind, track.name, track.id.0),
        }))
    }
}

// --- import_midi --------------------------------------------------------------

/// Input for `import_midi`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ImportMidiInput {
    /// Path to a Standard MIDI File.
    path: String,
    /// Timeline position (ticks, 960 PPQ) to place imported clips at.
    #[serde(default)]
    at: i64,
}

struct ImportMidi;

impl Tool for ImportMidi {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "import_midi",
            description: "Import a .mid file: each MIDI track becomes a new project \
                          track with one clip at the given position. Also imports \
                          the file's tempo map entries.",
            params_schema: schema::<ImportMidiInput>(),
        }
    }

    fn call(&self, ctx: &mut ProjectCtx, input: Value) -> Result<Value, ToolError> {
        let input: ImportMidiInput = parse(input)?;
        let bytes = std::fs::read(&input.path)
            .map_err(|e| ToolError::invalid(format!("cannot read {}: {e}", input.path)))?;
        let song = musicos_midi::import_smf(&bytes).map_err(ToolError::invalid)?;

        for &(at, tempo) in song.tempo_map.entries() {
            ctx.commit(Command::SetTempo { at, tempo })?;
        }
        let mut created = Vec::new();
        for (i, (name, pattern)) in song.tracks.iter().enumerate() {
            let track_name = name
                .clone()
                .unwrap_or_else(|| format!("Imported {}", i + 1));
            ctx.commit(Command::CreateTrack {
                name: track_name.clone(),
                kind: TrackKind::Midi,
            })?;
            let track_id = ctx.state().tracks.last().expect("just created").id;
            ctx.commit(Command::InsertClip {
                track: track_id,
                name: track_name.clone(),
                pattern: pattern.clone(),
                at: Tick(input.at),
            })?;
            created.push(json!({
                "track_id": track_id.0, "name": track_name, "notes": pattern.notes().len(),
            }));
        }
        Ok(json!({
            "tracks": created,
            "summary": format!(
                "imported {} track(s) from {} at tick {}",
                ctx.state().tracks.len(), input.path, input.at
            ),
        }))
    }
}

// --- set_tempo ------------------------------------------------------------------

/// Input for `set_tempo`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SetTempoInput {
    /// Tempo in beats per minute.
    bpm: f64,
    /// Timeline position (ticks). Defaults to 0 (whole-project tempo).
    #[serde(default)]
    at: i64,
}

struct SetTempo;

impl Tool for SetTempo {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "set_tempo",
            description: "Set the tempo (BPM) at a timeline position (default: start).",
            params_schema: schema::<SetTempoInput>(),
        }
    }

    fn call(&self, ctx: &mut ProjectCtx, input: Value) -> Result<Value, ToolError> {
        let input: SetTempoInput = parse(input)?;
        if !(input.bpm.is_finite() && (1.0..=1000.0).contains(&input.bpm)) {
            return Err(ToolError::invalid("bpm must be within 1..=1000"));
        }
        ctx.commit(Command::SetTempo {
            at: Tick(input.at),
            tempo: Tempo::from_bpm(input.bpm),
        })?;
        Ok(json!({
            "summary": format!("tempo set to {:.1} BPM at tick {}", input.bpm, input.at),
        }))
    }
}

// --- remove_track ---------------------------------------------------------------

/// Input for `remove_track`.
#[derive(Debug, Deserialize, JsonSchema)]
struct RemoveTrackInput {
    /// Id of the track to remove (see `get_project_summary`).
    track_id: u64,
}

struct RemoveTrack;

impl Tool for RemoveTrack {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "remove_track",
            description: "Remove a track and all clips on it. Undoable via undo.",
            params_schema: schema::<RemoveTrackInput>(),
        }
    }

    fn call(&self, ctx: &mut ProjectCtx, input: Value) -> Result<Value, ToolError> {
        let input: RemoveTrackInput = parse(input)?;
        ctx.commit(Command::RemoveTrack {
            track: TrackId(input.track_id),
        })?;
        Ok(json!({ "summary": format!("removed track {}", input.track_id) }))
    }
}

// --- undo -----------------------------------------------------------------------

/// Input for `undo`.
#[derive(Debug, Deserialize, JsonSchema)]
struct UndoInput {}

struct Undo;

impl Tool for Undo {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "undo",
            description: "Undo the most recent transaction recorded in the project log.",
            params_schema: schema::<UndoInput>(),
        }
    }

    fn call(&self, ctx: &mut ProjectCtx, input: Value) -> Result<Value, ToolError> {
        let UndoInput {} = parse(input)?;
        // Cross-invocation undo: rebuild session history from the persisted log
        // (docs/08 §4), then undo the last transaction.
        let log = ctx.store_log()?;
        let Some(last) = log.last() else {
            return Ok(json!({ "undone": false, "summary": "nothing to undo" }));
        };
        let inverse = musicos_project_model::Event::inverse_transaction(&last.txn.events);
        let mut state = ctx.state().clone();
        for ev in &inverse {
            state.apply_event(ev).map_err(ToolError::domain)?;
        }
        ctx.replace_state_and_pop_log(state)?;
        Ok(json!({
            "undone": true,
            "summary": format!("undid: {}", summarize_command(&last.txn.command)),
        }))
    }
}

fn summarize_command(cmd: &Command) -> String {
    match cmd {
        Command::RenameProject { name } => format!("rename project to '{name}'"),
        Command::CreateTrack { name, .. } => format!("create track '{name}'"),
        Command::RenameTrack { track, name } => format!("rename track {} to '{name}'", track.0),
        Command::RemoveTrack { track } => format!("remove track {}", track.0),
        Command::InsertClip { name, at, .. } => format!("insert clip '{name}' at {}", at.0),
        Command::RemoveClip { clip } => format!("remove clip {}", clip.0),
        Command::MoveClip { clip, at } => format!("move clip {} to {}", clip.0, at.0),
        Command::SetTempo { at, tempo } => {
            format!("set tempo {:.1} BPM at {}", tempo.bpm(), at.0)
        }
        Command::SetTrackGain { track, gain_db } => {
            format!("set track {} gain {gain_db:+.1} dB", track.0)
        }
        Command::SetTrackPan { track, pan } => format!("set track {} pan {pan:+.2}", track.0),
        Command::SetTrackMute { track, muted } => {
            format!(
                "{} track {}",
                if *muted { "mute" } else { "unmute" },
                track.0
            )
        }
        _ => "command".to_string(),
    }
}

impl ProjectCtx {
    fn store_log(&self) -> Result<Vec<musicos_storage::LogRecord>, ToolError> {
        self.store.read_log().map_err(ToolError::storage)
    }

    /// Applies an undone state: persists it and truncates the last log record.
    fn replace_state_and_pop_log(
        &mut self,
        state: musicos_project_model::ProjectState,
    ) -> Result<(), ToolError> {
        self.store.save_state(&state).map_err(ToolError::storage)?;
        self.store.pop_log().map_err(ToolError::storage)?;
        self.session = ProjectSession::from_state(state);
        Ok(())
    }
}

// --- render_song ----------------------------------------------------------------

/// Input for `render_song`.
#[derive(Debug, Deserialize, JsonSchema)]
struct RenderSongInput {
    /// Output WAV path.
    output: String,
    /// Sample rate in Hz (default 48000).
    #[serde(default)]
    sample_rate: Option<u32>,
}

struct RenderSong;

impl Tool for RenderSong {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "render_song",
            description: "Render the project to a 16-bit stereo WAV file using the \
                          built-in synthesizer. Deterministic per platform. Slower \
                          than analysis tools; call once per iteration.",
            params_schema: schema::<RenderSongInput>(),
        }
    }

    fn call(&self, ctx: &mut ProjectCtx, input: Value) -> Result<Value, ToolError> {
        let input: RenderSongInput = parse(input)?;
        let mut opts = musicos_render::RenderOptions::default();
        if let Some(rate) = input.sample_rate {
            if !(8_000..=192_000).contains(&rate) {
                return Err(ToolError::invalid(
                    "sample_rate must be within 8000..=192000",
                ));
            }
            opts.sample_rate = rate;
        }
        let path = std::path::PathBuf::from(&input.output);
        let report =
            musicos_render::render_to_wav(ctx.state(), &opts, &path).map_err(|e| ToolError {
                code: "E_RENDER",
                message: e.to_string(),
            })?;
        Ok(json!({
            "output": input.output,
            "seconds": report.seconds,
            "frames": report.frames,
            "peak": report.peak,
            "summary": format!(
                "rendered {:.1}s ({} frames, peak {:.2}) -> {}",
                report.seconds, report.frames, report.peak, input.output
            ),
        }))
    }
}

// --- mix: gain / pan / mute ------------------------------------------------------

/// Input for `set_track_mix`.
#[derive(Debug, Deserialize, JsonSchema)]
struct SetTrackMixInput {
    /// Target track id (see `get_project_summary`).
    track_id: u64,
    /// Gain in dB (\u221296.0..=12.0). Omit to leave unchanged.
    #[serde(default)]
    gain_db: Option<f32>,
    /// Pan (\u22121.0 left ..= 1.0 right). Omit to leave unchanged.
    #[serde(default)]
    pan: Option<f32>,
    /// Mute state. Omit to leave unchanged.
    #[serde(default)]
    muted: Option<bool>,
}

struct SetTrackMix;

impl Tool for SetTrackMix {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "set_track_mix",
            description: "Set a track's mix parameters: gain_db, pan, and/or muted. \
                          Each provided field is one undoable transaction.",
            params_schema: schema::<SetTrackMixInput>(),
        }
    }

    fn call(&self, ctx: &mut ProjectCtx, input: Value) -> Result<Value, ToolError> {
        let input: SetTrackMixInput = parse(input)?;
        let track = TrackId(input.track_id);
        let mut changes = Vec::new();
        if let Some(gain_db) = input.gain_db {
            ctx.commit(Command::SetTrackGain { track, gain_db })?;
            changes.push(format!("gain {gain_db:+.1} dB"));
        }
        if let Some(pan) = input.pan {
            ctx.commit(Command::SetTrackPan { track, pan })?;
            changes.push(format!("pan {pan:+.2}"));
        }
        if let Some(muted) = input.muted {
            ctx.commit(Command::SetTrackMute { track, muted })?;
            changes.push(if muted {
                "muted".into()
            } else {
                "unmuted".to_string()
            });
        }
        if changes.is_empty() {
            return Err(ToolError::invalid(
                "provide at least one of gain_db, pan, muted",
            ));
        }
        Ok(json!({
            "track_id": input.track_id,
            "summary": format!("track {}: {}", input.track_id, changes.join(", ")),
        }))
    }
}

// --- composition: generate_chords / melody / bass / drums -----------------------

fn parse_scale(key: &str, scale: &str) -> Result<Scale, ToolError> {
    let tonic = parse_note_name(key).map_err(ToolError::invalid)?;
    let kind = parse_scale_kind(scale).map_err(ToolError::invalid)?;
    Ok(Scale { tonic, kind })
}

fn parse_progression(symbols: &[String]) -> Result<Vec<Chord>, ToolError> {
    if symbols.is_empty() {
        return Err(ToolError::invalid("progression must not be empty"));
    }
    symbols
        .iter()
        .map(|s| Chord::parse(s).map_err(ToolError::invalid))
        .collect()
}

/// Creates a MIDI track holding one clip with the pattern; returns ids.
fn insert_generated(
    ctx: &mut ProjectCtx,
    track_name: &str,
    clip_name: &str,
    pattern: Pattern,
    at: i64,
) -> Result<(u64, usize), ToolError> {
    ctx.commit(Command::CreateTrack {
        name: track_name.to_string(),
        kind: TrackKind::Midi,
    })?;
    let track = ctx.state().tracks.last().expect("just created").id;
    let notes = pattern.notes().len();
    ctx.commit(Command::InsertClip {
        track,
        name: clip_name.to_string(),
        pattern,
        at: Tick(at),
    })?;
    Ok((track.0, notes))
}

/// Input for `generate_chords`.
#[derive(Debug, Deserialize, JsonSchema)]
struct GenerateChordsInput {
    /// Key root note name, e.g. "C", "F#", "Bb".
    key: String,
    /// Scale: `major` | `minor` | `harmonic_minor` | `melodic_minor` |
    /// `dorian` | `mixolydian` | `major_pentatonic` | `minor_pentatonic`.
    /// Default "major".
    #[serde(default)]
    scale: Option<String>,
    /// Number of bars (one chord per bar, 4/4). Default 8.
    #[serde(default)]
    bars: Option<usize>,
    /// Random seed — same inputs and seed always give the same music. Default 0.
    #[serde(default)]
    seed: Option<u64>,
    /// Track name. Default "Chords".
    #[serde(default)]
    track_name: Option<String>,
    /// Timeline position in ticks (960 PPQ). Default 0.
    #[serde(default)]
    at: Option<i64>,
}

struct GenerateChords;

impl Tool for GenerateChords {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "generate_chords",
            description: "Compose a chord progression with functional-harmony structure \
                          (starts on I, cadences V->I) and place it as block chords on a \
                          new track. Returns the progression symbols — pass them to \
                          generate_melody / generate_bass to build on the same structure.",
            params_schema: schema::<GenerateChordsInput>(),
        }
    }

    fn call(&self, ctx: &mut ProjectCtx, input: Value) -> Result<Value, ToolError> {
        let input: GenerateChordsInput = parse(input)?;
        let bars = input.bars.unwrap_or(8).clamp(1, 256);
        let scale = parse_scale(&input.key, input.scale.as_deref().unwrap_or("major"))?;
        let progression = generate_chords(scale, bars, Seed(input.seed.unwrap_or(0)));
        let symbols: Vec<String> = progression.iter().map(Chord::symbol).collect();
        let pattern = chords_to_pattern(&progression, 3, 78);
        let (track_id, notes) = insert_generated(
            ctx,
            input.track_name.as_deref().unwrap_or("Chords"),
            "progression",
            pattern,
            input.at.unwrap_or(0),
        )?;
        Ok(json!({
            "progression": symbols,
            "track_id": track_id,
            "notes": notes,
            "summary": format!(
                "chords on track {track_id}: {} ({bars} bars)",
                symbols.join(" - ")
            ),
        }))
    }
}

/// Input for `generate_melody`.
#[derive(Debug, Deserialize, JsonSchema)]
struct GenerateMelodyInput {
    /// Chord symbols, one per bar, e.g. `["Am","F","C","G"]`.
    progression: Vec<String>,
    /// Key root note name for scale-step passing tones, e.g. "A".
    key: String,
    /// Scale (see `generate_chords`). Default "major".
    #[serde(default)]
    scale: Option<String>,
    /// Random seed. Default 0.
    #[serde(default)]
    seed: Option<u64>,
    /// Track name. Default "Melody".
    #[serde(default)]
    track_name: Option<String>,
    /// Timeline position in ticks. Default 0.
    #[serde(default)]
    at: Option<i64>,
}

struct GenerateMelody;

impl Tool for GenerateMelody {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "generate_melody",
            description: "Compose a melody over a chord progression (chord tones on \
                          strong beats, scale steps between) on a new track. Use the \
                          progression returned by generate_chords, or write your own.",
            params_schema: schema::<GenerateMelodyInput>(),
        }
    }

    fn call(&self, ctx: &mut ProjectCtx, input: Value) -> Result<Value, ToolError> {
        let input: GenerateMelodyInput = parse(input)?;
        let progression = parse_progression(&input.progression)?;
        let scale = parse_scale(&input.key, input.scale.as_deref().unwrap_or("major"))?;
        let pattern = generate_melody(&progression, scale, Seed(input.seed.unwrap_or(0)));
        let (track_id, notes) = insert_generated(
            ctx,
            input.track_name.as_deref().unwrap_or("Melody"),
            "melody",
            pattern,
            input.at.unwrap_or(0),
        )?;
        Ok(json!({
            "track_id": track_id,
            "notes": notes,
            "summary": format!("melody on track {track_id}: {notes} notes over {} bars",
                input.progression.len()),
        }))
    }
}

/// Input for `generate_bass`.
#[derive(Debug, Deserialize, JsonSchema)]
struct GenerateBassInput {
    /// Chord symbols, one per bar.
    progression: Vec<String>,
    /// Random seed. Default 0.
    #[serde(default)]
    seed: Option<u64>,
    /// Track name. Default "Bass".
    #[serde(default)]
    track_name: Option<String>,
    /// Timeline position in ticks. Default 0.
    #[serde(default)]
    at: Option<i64>,
}

struct GenerateBass;

impl Tool for GenerateBass {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "generate_bass",
            description: "Compose a bassline following a chord progression (root on the \
                          downbeat, seeded root/fifth/octave movement) on a new track.",
            params_schema: schema::<GenerateBassInput>(),
        }
    }

    fn call(&self, ctx: &mut ProjectCtx, input: Value) -> Result<Value, ToolError> {
        let input: GenerateBassInput = parse(input)?;
        let progression = parse_progression(&input.progression)?;
        let pattern = generate_bass(&progression, Seed(input.seed.unwrap_or(0)));
        let (track_id, notes) = insert_generated(
            ctx,
            input.track_name.as_deref().unwrap_or("Bass"),
            "bass",
            pattern,
            input.at.unwrap_or(0),
        )?;
        Ok(json!({
            "track_id": track_id,
            "notes": notes,
            "summary": format!("bass on track {track_id}: {notes} notes"),
        }))
    }
}

/// Input for `generate_drums`.
#[derive(Debug, Deserialize, JsonSchema)]
struct GenerateDrumsInput {
    /// Number of bars (4/4). Default 8.
    #[serde(default)]
    bars: Option<usize>,
    /// Style: `basic` | `four_on_floor` | `lofi`. Default "basic".
    #[serde(default)]
    style: Option<String>,
    /// Random seed. Default 0.
    #[serde(default)]
    seed: Option<u64>,
    /// Track name. Default "Drums".
    #[serde(default)]
    track_name: Option<String>,
    /// Timeline position in ticks. Default 0.
    #[serde(default)]
    at: Option<i64>,
}

struct GenerateDrums;

impl Tool for GenerateDrums {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "generate_drums",
            description: "Compose a drum pattern (GM keys: kick 36, snare 38, hat 42) \
                          on a new track. Styles: basic, four_on_floor, lofi.",
            params_schema: schema::<GenerateDrumsInput>(),
        }
    }

    fn call(&self, ctx: &mut ProjectCtx, input: Value) -> Result<Value, ToolError> {
        let input: GenerateDrumsInput = parse(input)?;
        let bars = input.bars.unwrap_or(8).clamp(1, 256);
        let style = DrumStyle::parse(input.style.as_deref().unwrap_or("basic"))
            .map_err(|s| ToolError::invalid(format!("unknown drum style '{s}'")))?;
        let pattern = generate_drums(bars, style, Seed(input.seed.unwrap_or(0)));
        let (track_id, notes) = insert_generated(
            ctx,
            input.track_name.as_deref().unwrap_or("Drums"),
            "drums",
            pattern,
            input.at.unwrap_or(0),
        )?;
        Ok(json!({
            "track_id": track_id,
            "notes": notes,
            "summary": format!("drums on track {track_id}: {notes} hits, {bars} bars"),
        }))
    }
}

/// The canonical tool registry.
pub struct Registry {
    tools: Vec<Box<dyn Tool>>,
}

impl Registry {
    /// All built-in tools.
    pub fn new() -> Registry {
        Registry {
            tools: vec![
                Box::new(GetProjectSummary),
                Box::new(AddTrack),
                Box::new(RemoveTrack),
                Box::new(ImportMidi),
                Box::new(SetTempo),
                Box::new(RenderSong),
                Box::new(SetTrackMix),
                Box::new(GenerateChords),
                Box::new(GenerateMelody),
                Box::new(GenerateBass),
                Box::new(GenerateDrums),
                Box::new(Undo),
            ],
        }
    }

    /// Specs of every registered tool (the CLI/MCP surface).
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.iter().map(|t| t.spec()).collect()
    }

    /// Calls a tool by name.
    ///
    /// # Errors
    /// Returns `E_UNKNOWN_TOOL` for unknown names, or the tool's own error.
    pub fn call(&self, name: &str, ctx: &mut ProjectCtx, input: Value) -> Result<Value, ToolError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.spec().name == name)
            .ok_or(ToolError {
                code: "E_UNKNOWN_TOOL",
                message: format!("no tool '{name}'"),
            })?;
        tool.call(ctx, input)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Registry::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicos_core_types::ProjectId;
    use musicos_project_model::ProjectState;

    fn ctx(name: &str) -> (ProjectCtx, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("musicos-tools-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = ProjectState::new(ProjectId(1), "T");
        BundleStore::create(&dir, &state).unwrap();
        (ProjectCtx::open(&dir, "user:test").unwrap(), dir)
    }

    #[test]
    fn registry_exposes_schemas_and_dispatches() {
        let registry = Registry::new();
        let specs = registry.specs();
        assert!(specs.iter().any(|s| s.name == "add_track"));
        for s in &specs {
            assert!(
                s.params_schema.is_object(),
                "{} schema must be an object",
                s.name
            );
            assert!(!s.description.is_empty());
        }

        let (mut ctx, dir) = ctx("dispatch");
        let out = registry
            .call("add_track", &mut ctx, json!({ "name": "Drums" }))
            .unwrap();
        assert_eq!(out["track_id"], 0);
        let out = registry
            .call("get_project_summary", &mut ctx, json!({}))
            .unwrap();
        assert_eq!(out["tracks"].as_array().unwrap().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn errors_carry_stable_codes() {
        let registry = Registry::new();
        let (mut ctx, dir) = ctx("errors");
        let err = registry.call("nope", &mut ctx, json!({})).unwrap_err();
        assert_eq!(err.code, "E_UNKNOWN_TOOL");
        let err = registry
            .call(
                "add_track",
                &mut ctx,
                json!({ "name": "x", "kind": "vocal" }),
            )
            .unwrap_err();
        assert_eq!(err.code, "E_INVALID_INPUT");
        let err = registry
            .call("remove_track", &mut ctx, json!({ "track_id": 99 }))
            .unwrap_err();
        assert_eq!(err.code, "E_DOMAIN");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn undo_tool_works_across_reopened_contexts() {
        let (mut c, dir) = ctx("undo");
        let registry = Registry::new();
        registry
            .call("add_track", &mut c, json!({ "name": "A" }))
            .unwrap();
        drop(c); // simulate a separate CLI invocation
        let mut c2 = ProjectCtx::open(&dir, "user:test").unwrap();
        assert_eq!(c2.state().tracks.len(), 1);
        let out = registry.call("undo", &mut c2, json!({})).unwrap();
        assert_eq!(out["undone"], true);
        assert_eq!(c2.state().tracks.len(), 0);
        let out = registry.call("undo", &mut c2, json!({})).unwrap();
        assert_eq!(out["undone"], false);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
