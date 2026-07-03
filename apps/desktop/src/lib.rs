//! MusicOS desktop client (Tauri shell over the shared services).
//!
//! The desktop app is **just another client** (docs/00 §4, docs/02): every
//! command below is a thin wrapper over the same tool registry the CLI and
//! MCP server use — no business logic lives here. The UI is a plain static
//! page (`ui/`) served by Tauri; a React front end can replace it without
//! touching this crate's command surface.

use std::sync::Mutex;

use musicos_core_types::ProjectId;
use musicos_project_model::ProjectState;
use musicos_storage::BundleStore;
use musicos_tools::{ProjectCtx, Registry};
use serde_json::{json, Value};

/// The one live playback session (the desktop is a single-transport app).
static PLAYBACK: Mutex<Option<musicos_audio_engine::PlaybackSession>> = Mutex::new(None);

/// Latest AI run transcript/state, polled by the UI.
static AI_RUN: Mutex<AiRun> = Mutex::new(AiRun {
    running: false,
    transcript: String::new(),
});

struct AiRun {
    running: bool,
    transcript: String,
}

/// Actor recorded in project logs for desktop-driven commands.
const ACTOR: &str = "user:desktop";

/// Lists every registry tool (name, description, input schema).
fn tools_impl() -> Value {
    let specs: Vec<Value> = Registry::new()
        .specs()
        .iter()
        .map(|s| json!({ "name": s.name, "description": s.description, "params": s.params_schema }))
        .collect();
    json!(specs)
}

/// Calls a registry tool against a project bundle.
fn call_impl(project: &str, tool: &str, input: Value) -> Result<Value, String> {
    let mut ctx =
        ProjectCtx::open(std::path::Path::new(project), ACTOR).map_err(|e| e.to_string())?;
    Registry::new()
        .call(tool, &mut ctx, input)
        .map_err(|e| e.to_string())
}

/// Creates a new project bundle.
fn create_project_impl(path: &str, name: &str) -> Result<Value, String> {
    let id = ProjectId(name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3)
    }));
    BundleStore::create(std::path::Path::new(path), &ProjectState::new(id, name))
        .map_err(|e| e.to_string())?;
    Ok(json!({ "path": path, "name": name }))
}

#[tauri::command]
fn tools() -> Value {
    tools_impl()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // tauri commands receive owned args
fn call(project: String, tool: String, input: Value) -> Result<Value, String> {
    call_impl(&project, &tool, input)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // tauri commands receive owned args
fn create_project(path: String, name: String) -> Result<Value, String> {
    create_project_impl(&path, &name)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // tauri commands receive owned args
fn play(project: String, from_bar: u64) -> Result<Value, String> {
    let store = BundleStore::open(std::path::Path::new(&project)).map_err(|e| e.to_string())?;
    let state = store.load_state().map_err(|e| e.to_string())?;
    let session = musicos_audio_engine::start(&state, from_bar).map_err(|e| e.to_string())?;
    let (done, total) = session.progress();
    *PLAYBACK.lock().expect("playback lock") = Some(session);
    Ok(json!({ "playing": true, "done": done, "total": total }))
}

#[tauri::command]
fn stop_playback() -> Value {
    let stopped = PLAYBACK
        .lock()
        .expect("playback lock")
        .take()
        .map(|mut session| session.stop_and_wait())
        .is_some();
    json!({ "stopped": stopped })
}

#[tauri::command]
fn playback_status() -> Value {
    let mut guard = PLAYBACK.lock().expect("playback lock");
    match guard.as_ref() {
        Some(session) => {
            let (done, total) = session.progress();
            if session.is_finished() {
                *guard = None;
                json!({ "playing": false, "done": done, "total": total })
            } else {
                json!({ "playing": true, "done": done, "total": total })
            }
        }
        None => json!({ "playing": false }),
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // tauri commands receive owned args
fn ai_run(project: String, brief: String) -> Result<Value, String> {
    {
        let mut run = AI_RUN.lock().expect("ai lock");
        if run.running {
            return Err("an AI run is already in progress".into());
        }
        run.running = true;
        run.transcript = "starting Claude (subscription)...".into();
    }
    std::thread::spawn(move || {
        let result = musicos_ai_providers::find_server_binary()
            .map_err(|e| e.to_string())
            .and_then(|server_bin| {
                musicos_ai_providers::SubscriptionRunner {
                    server_bin,
                    project: std::path::PathBuf::from(&project),
                }
                .run_captured(&brief)
                .map_err(|e| e.to_string())
            });
        let mut run = AI_RUN.lock().expect("ai lock");
        run.running = false;
        run.transcript = match result {
            Ok(transcript) => transcript,
            Err(e) => format!("AI run failed: {e}"),
        };
    });
    Ok(json!({ "started": true }))
}

#[tauri::command]
fn ai_status() -> Value {
    let run = AI_RUN.lock().expect("ai lock");
    json!({ "running": run.running, "transcript": run.transcript })
}

/// Launches the desktop app.
///
/// # Panics
/// Panics if the Tauri runtime fails to start (no display, broken webview).
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            tools,
            call,
            create_project,
            play,
            stop_playback,
            playback_status,
            ai_run,
            ai_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running MusicOS desktop");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_commands_share_the_registry() {
        let listed = tools_impl();
        assert!(listed
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "render_song"));

        let dir = std::env::temp_dir().join(format!("musicos-desktop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.display().to_string();
        create_project_impl(&path, "Desk").unwrap();
        let out = call_impl(&path, "add_track", json!({ "name": "Keys" })).unwrap();
        assert_eq!(out["track_id"], 0);
        let summary = call_impl(&path, "get_project_summary", json!({})).unwrap();
        assert_eq!(summary["tracks"].as_array().unwrap().len(), 1);
        assert!(call_impl(&path, "nope", json!({})).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
