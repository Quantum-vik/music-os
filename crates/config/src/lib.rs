//! Layered configuration: global, workspace, project, and runtime.
//!
//! Implements FR-S3 from `docs/01_Product_Requirements.md` per the strategy in
//! `docs/02_System_Architecture.md` §7: four layers are merged field-wise, each
//! later layer overriding only the fields it explicitly sets:
//!
//! 1. **Global** — `$MUSICOS_CONFIG_DIR/config.toml` when that variable is set,
//!    otherwise `~/.config/musicos/config.toml`.
//! 2. **Workspace** — `musicos.workspace.toml` in the nearest ancestor of the
//!    current directory that contains one.
//! 3. **Project** — `<project_dir>/musicos.toml`.
//! 4. **Runtime** — `MUSICOS_*` environment variables.
//!
//! Missing files are skipped silently; malformed files or unparseable env
//! values produce warnings on [`LoadedConfig`] but never fail the load.

use std::env;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// AI-related settings (provider selection and conversation limits).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiConfig {
    /// AI provider identifier (e.g. `"anthropic"`). `None` selects the default
    /// provider chosen by the AI runtime.
    pub provider: Option<String>,
    /// Model identifier passed to the provider.
    pub model: String,
    /// Maximum number of agent turns per task before the runtime stops.
    pub max_turns: u32,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: None,
            model: "claude-opus-4-8".to_owned(),
            max_turns: 16,
        }
    }
}

/// Offline render settings.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderConfig {
    /// Output sample rate in Hz.
    pub sample_rate: u32,
    /// Extra tail rendered after the last event, in seconds, so reverb and
    /// delay tails are not cut off.
    pub tail_seconds: f32,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            tail_seconds: 0.5,
        }
    }
}

/// Fully-resolved MusicOS configuration.
///
/// Produced by [`Config::load`]; every field has a documented default so a
/// system with no configuration files at all still works.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    /// AI runtime settings.
    pub ai: AiConfig,
    /// Offline render settings.
    pub render: RenderConfig,
}

/// The result of loading configuration: the merged [`Config`] plus any
/// non-fatal warnings encountered along the way.
#[derive(Debug, Clone, Default)]
pub struct LoadedConfig {
    /// The merged configuration.
    pub config: Config,
    /// Human-readable warnings (malformed files, unparseable env values).
    pub warnings: Vec<String>,
}

/// Mirror of [`AiConfig`] where every field is optional.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialAiConfig {
    provider: Option<String>,
    model: Option<String>,
    max_turns: Option<u32>,
}

/// Mirror of [`RenderConfig`] where every field is optional.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialRenderConfig {
    sample_rate: Option<u32>,
    tail_seconds: Option<f32>,
}

/// Mirror of [`Config`] where every field is optional; one layer of the merge.
#[derive(Debug, Clone, Default, Deserialize)]
struct PartialConfig {
    #[serde(default)]
    ai: PartialAiConfig,
    #[serde(default)]
    render: PartialRenderConfig,
}

impl PartialConfig {
    /// Overrides on `config` exactly the fields this layer sets.
    fn apply(&self, config: &mut Config) {
        if let Some(provider) = &self.ai.provider {
            config.ai.provider = Some(provider.clone());
        }
        if let Some(model) = &self.ai.model {
            config.ai.model.clone_from(model);
        }
        if let Some(max_turns) = self.ai.max_turns {
            config.ai.max_turns = max_turns;
        }
        if let Some(sample_rate) = self.render.sample_rate {
            config.render.sample_rate = sample_rate;
        }
        if let Some(tail_seconds) = self.render.tail_seconds {
            config.render.tail_seconds = tail_seconds;
        }
    }
}

impl Config {
    /// Loads configuration by merging the global, workspace, project, and
    /// runtime layers, in that order (later layers win, field-wise).
    ///
    /// `project_dir` is the project root whose `musicos.toml` supplies the
    /// project layer; pass `None` when no project is open. Missing files are
    /// skipped silently; malformed files and unparseable environment values
    /// add a warning to the returned [`LoadedConfig`] instead of failing.
    #[must_use]
    pub fn load(project_dir: Option<&Path>) -> LoadedConfig {
        let mut loaded = LoadedConfig::default();

        // 1. Global.
        if let Some(path) = global_config_path() {
            apply_file(&path, &mut loaded);
        }

        // 2. Workspace: nearest ancestor with musicos.workspace.toml.
        if let Some(path) = find_workspace_file() {
            apply_file(&path, &mut loaded);
        }

        // 3. Project.
        if let Some(dir) = project_dir {
            apply_file(&dir.join("musicos.toml"), &mut loaded);
        }

        // 4. Runtime environment.
        apply_env(&mut loaded);

        loaded
    }
}

/// Resolves the global config file path, honouring `MUSICOS_CONFIG_DIR`.
fn global_config_path() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("MUSICOS_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("config.toml"));
    }
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/musicos/config.toml"))
}

/// Walks up from the current directory looking for `musicos.workspace.toml`.
fn find_workspace_file() -> Option<PathBuf> {
    let mut dir = env::current_dir().ok()?;
    loop {
        let candidate = dir.join("musicos.workspace.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Reads and applies one TOML layer. A missing file is skipped silently; an
/// unreadable or malformed file adds a warning and changes nothing.
fn apply_file(path: &Path, loaded: &mut LoadedConfig) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => {
            loaded
                .warnings
                .push(format!("failed to read {}: {err}", path.display()));
            return;
        }
    };
    match toml::from_str::<PartialConfig>(&text) {
        Ok(partial) => partial.apply(&mut loaded.config),
        Err(err) => loaded
            .warnings
            .push(format!("malformed config {}: {err}", path.display())),
    }
}

/// Applies the runtime environment layer (`MUSICOS_*` variables). Parse
/// failures warn and keep the prior value.
fn apply_env(loaded: &mut LoadedConfig) {
    if let Ok(provider) = env::var("MUSICOS_AI_PROVIDER") {
        loaded.config.ai.provider = Some(provider);
    }
    if let Ok(model) = env::var("MUSICOS_AI_MODEL") {
        loaded.config.ai.model = model;
    }
    apply_env_parsed("MUSICOS_AI_MAX_TURNS", loaded, |config, value| {
        config.ai.max_turns = value;
    });
    apply_env_parsed("MUSICOS_RENDER_SAMPLE_RATE", loaded, |config, value| {
        config.render.sample_rate = value;
    });
}

/// Parses one numeric env var and applies it via `set`; a parse failure adds a
/// warning and leaves the prior value untouched.
fn apply_env_parsed(name: &str, loaded: &mut LoadedConfig, set: impl FnOnce(&mut Config, u32)) {
    let Ok(raw) = env::var(name) else { return };
    match raw.parse::<u32>() {
        Ok(value) => set(&mut loaded.config, value),
        Err(err) => loaded.warnings.push(format!("invalid {name}={raw}: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Serializes env-mutating tests; env vars are process-global.
    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Env vars read by `Config::load`, cleared around every test.
    const ENV_VARS: &[&str] = &[
        "MUSICOS_CONFIG_DIR",
        "MUSICOS_AI_PROVIDER",
        "MUSICOS_AI_MODEL",
        "MUSICOS_AI_MAX_TURNS",
        "MUSICOS_RENDER_SAMPLE_RATE",
    ];

    fn clear_env() {
        for var in ENV_VARS {
            env::remove_var(var);
        }
    }

    /// Fresh temp dir namespaced by process id and test name.
    fn temp_dir(test: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("musicos-config-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// Points the global layer at an empty directory so a developer's real
    /// `~/.config/musicos/config.toml` cannot leak into tests.
    fn isolate_global(test: &str) -> PathBuf {
        let dir = temp_dir(&format!("{test}-global"));
        env::set_var("MUSICOS_CONFIG_DIR", &dir);
        dir
    }

    #[test]
    fn defaults_when_nothing_exists() {
        let _guard = env_lock();
        clear_env();
        isolate_global("defaults");

        let loaded = Config::load(None);
        assert_eq!(loaded.config, Config::default());
        assert_eq!(loaded.config.ai.model, "claude-opus-4-8");
        assert_eq!(loaded.config.ai.max_turns, 16);
        assert_eq!(loaded.config.render.sample_rate, 48_000);
        assert!(loaded.warnings.is_empty());
        clear_env();
    }

    #[test]
    fn project_overrides_global() {
        let _guard = env_lock();
        clear_env();
        let global = isolate_global("project-overrides");
        fs::write(
            global.join("config.toml"),
            "[ai]\nmodel = \"global-model\"\nmax_turns = 5\n",
        )
        .expect("write global config");

        let project = temp_dir("project-overrides-project");
        fs::write(
            project.join("musicos.toml"),
            "[ai]\nmodel = \"project-model\"\n",
        )
        .expect("write project config");

        let loaded = Config::load(Some(&project));
        // Project layer wins where it sets a field...
        assert_eq!(loaded.config.ai.model, "project-model");
        // ...but the global layer's other fields survive the merge.
        assert_eq!(loaded.config.ai.max_turns, 5);
        assert!(loaded.warnings.is_empty());
        clear_env();
    }

    #[test]
    fn env_overrides_file() {
        let _guard = env_lock();
        clear_env();
        isolate_global("env-overrides");
        let project = temp_dir("env-overrides-project");
        fs::write(
            project.join("musicos.toml"),
            "[ai]\nmodel = \"file-model\"\n[render]\nsample_rate = 22050\n",
        )
        .expect("write project config");

        env::set_var("MUSICOS_AI_MODEL", "env-model");
        env::set_var("MUSICOS_AI_PROVIDER", "env-provider");
        env::set_var("MUSICOS_RENDER_SAMPLE_RATE", "96000");
        let loaded = Config::load(Some(&project));
        assert_eq!(loaded.config.ai.model, "env-model");
        assert_eq!(loaded.config.ai.provider.as_deref(), Some("env-provider"));
        assert_eq!(loaded.config.render.sample_rate, 96_000);
        assert!(loaded.warnings.is_empty());
        clear_env();
    }

    #[test]
    fn env_parse_failure_warns_and_keeps_prior_value() {
        let _guard = env_lock();
        clear_env();
        isolate_global("env-parse");

        env::set_var("MUSICOS_AI_MAX_TURNS", "not-a-number");
        let loaded = Config::load(None);
        assert_eq!(loaded.config.ai.max_turns, 16);
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.warnings[0].contains("MUSICOS_AI_MAX_TURNS"));
        clear_env();
    }

    #[test]
    fn malformed_toml_warns_but_loads() {
        let _guard = env_lock();
        clear_env();
        isolate_global("malformed");
        let project = temp_dir("malformed-project");
        fs::write(project.join("musicos.toml"), "this is [not valid toml").expect("write file");

        let loaded = Config::load(Some(&project));
        assert_eq!(loaded.config, Config::default());
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.warnings[0].contains("musicos.toml"));
        clear_env();
    }

    #[test]
    fn musicos_config_dir_is_honored() {
        let _guard = env_lock();
        clear_env();
        let dir = temp_dir("config-dir");
        fs::write(dir.join("config.toml"), "[render]\ntail_seconds = 2.5\n")
            .expect("write global config");
        env::set_var("MUSICOS_CONFIG_DIR", &dir);

        let loaded = Config::load(None);
        assert!((loaded.config.render.tail_seconds - 2.5).abs() < f32::EPSILON);
        assert!(loaded.warnings.is_empty());
        clear_env();
    }
}
