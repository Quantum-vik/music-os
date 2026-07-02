//! Language-model provider adapters (Claude subscription, Anthropic API).
//!
//! Two ways to bring a brain to MusicOS, the user's choice (`docs/06` §3):
//!
//! - [`SubscriptionRunner`] — delegates the whole agent loop to the local
//!   Claude Code CLI (`claude -p`), wired to our own MCP server. Reasoning is
//!   billed to the user's Claude subscription; no API keys touch MusicOS.
//! - [`AnthropicBackend`] — a raw Messages-API backend (Rust has no official
//!   Anthropic SDK) for the in-process loop in `musicos-ai-runtime`.
//!   Authenticates with `ANTHROPIC_API_KEY` (x-api-key) or
//!   `ANTHROPIC_AUTH_TOKEN` (OAuth bearer). Retries 429/5xx with backoff,
//!   honoring `retry-after`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use musicos_ai_runtime::{AgentError, ModelBackend};
use serde_json::{json, Value};

/// How the user wants the reasoning provided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Local Claude Code CLI on the user's subscription (no API keys).
    Subscription,
    /// Direct Anthropic API with `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN`.
    Api,
}

impl Provider {
    /// Resolves the provider: explicit choice > `MUSICOS_AI_PROVIDER` env >
    /// auto-detect (`claude` on PATH → subscription; API credentials in the
    /// environment → api).
    ///
    /// # Errors
    /// Returns a human-actionable error when neither path is available.
    pub fn resolve(explicit: Option<&str>) -> Result<Provider, ProviderError> {
        let choice = explicit
            .map(str::to_string)
            .or_else(|| std::env::var("MUSICOS_AI_PROVIDER").ok())
            .unwrap_or_else(|| "auto".to_string());
        match choice.as_str() {
            "subscription" | "sub" => Ok(Provider::Subscription),
            "api" => Ok(Provider::Api),
            "auto" => {
                if claude_cli_available() {
                    Ok(Provider::Subscription)
                } else if api_credentials_present() {
                    Ok(Provider::Api)
                } else {
                    Err(ProviderError::NoneAvailable)
                }
            }
            other => Err(ProviderError::Unknown(other.to_string())),
        }
    }
}

fn claude_cli_available() -> bool {
    Command::new("claude")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn api_credentials_present() -> bool {
    std::env::var("ANTHROPIC_API_KEY").is_ok_and(|v| !v.is_empty())
        || std::env::var("ANTHROPIC_AUTH_TOKEN").is_ok_and(|v| !v.is_empty())
}

/// Runs an agentic brief on the user's Claude subscription by spawning the
/// Claude Code CLI headless, with MusicOS's MCP server as its only tool source
/// (`--strict-mcp-config` keeps the user's other servers out — token
/// efficiency and least privilege).
pub struct SubscriptionRunner {
    /// Path to the `music-server` binary.
    pub server_bin: PathBuf,
    /// Project bundle the run is scoped to.
    pub project: PathBuf,
}

impl SubscriptionRunner {
    /// Streams the run to the user's terminal (stdout/stderr inherited).
    ///
    /// # Errors
    /// Fails if the `claude` CLI is missing or exits non-zero.
    pub fn run(&self, brief: &str) -> Result<(), ProviderError> {
        let mcp_config = json!({
            "mcpServers": {
                "musicos": {
                    "command": self.server_bin.display().to_string(),
                    "args": ["--project", self.project.display().to_string()],
                }
            }
        });
        let config_path =
            std::env::temp_dir().join(format!("musicos-mcp-{}.json", std::process::id()));
        std::fs::write(&config_path, mcp_config.to_string())
            .map_err(|e| ProviderError::Io(e.to_string()))?;

        let status = Command::new("claude")
            .arg("-p")
            .arg(brief)
            .arg("--mcp-config")
            .arg(&config_path)
            .arg("--strict-mcp-config")
            .arg("--allowedTools")
            .arg("mcp__musicos__*")
            .arg("--append-system-prompt")
            .arg(
                "Use the musicos MCP tools to fulfil the request. Start with \
                 get_project_summary; prefer the fewest tool calls; render only when asked.",
            )
            .status()
            .map_err(|e| ProviderError::Io(format!("failed to spawn claude: {e}")))?;
        let _ = std::fs::remove_file(&config_path);
        if status.success() {
            Ok(())
        } else {
            Err(ProviderError::ClaudeExit(status.code().unwrap_or(-1)))
        }
    }
}

/// Anthropic Messages API backend for the in-process agent loop.
pub struct AnthropicBackend {
    base_url: String,
    api_key: Option<String>,
    auth_token: Option<String>,
    max_retries: u32,
}

impl AnthropicBackend {
    /// Builds a backend from the environment.
    ///
    /// # Errors
    /// Fails when neither `ANTHROPIC_API_KEY` nor `ANTHROPIC_AUTH_TOKEN` is set.
    pub fn from_env() -> Result<AnthropicBackend, ProviderError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|v| !v.is_empty());
        let auth_token = std::env::var("ANTHROPIC_AUTH_TOKEN")
            .ok()
            .filter(|v| !v.is_empty());
        if api_key.is_none() && auth_token.is_none() {
            return Err(ProviderError::NoApiCredentials);
        }
        Ok(AnthropicBackend {
            base_url: std::env::var("ANTHROPIC_BASE_URL")
                .unwrap_or_else(|_| "https://api.anthropic.com".to_string()),
            api_key,
            auth_token,
            max_retries: 3,
        })
    }
}

impl ModelBackend for AnthropicBackend {
    fn complete(&self, body: &Value) -> Result<Value, AgentError> {
        let url = format!("{}/v1/messages", self.base_url);
        let mut attempt = 0u32;
        loop {
            let mut request = ureq::post(&url)
                .set("content-type", "application/json")
                .set("anthropic-version", "2023-06-01");
            if let Some(key) = &self.api_key {
                request = request.set("x-api-key", key);
            } else if let Some(token) = &self.auth_token {
                request = request
                    .set("authorization", &format!("Bearer {token}"))
                    .set("anthropic-beta", "oauth-2025-04-20");
            }

            match request.send_json(body.clone()) {
                Ok(response) => {
                    return response
                        .into_json::<Value>()
                        .map_err(|e| AgentError::Model(format!("malformed response: {e}")));
                }
                Err(ureq::Error::Status(code, response)) => {
                    let retryable = code == 429 || code >= 500;
                    if retryable && attempt < self.max_retries {
                        let wait = response
                            .header("retry-after")
                            .and_then(|v| v.parse::<u64>().ok())
                            .unwrap_or(2u64.pow(attempt));
                        std::thread::sleep(Duration::from_secs(wait.min(30)));
                        attempt += 1;
                        continue;
                    }
                    let detail = response.into_string().unwrap_or_default();
                    return Err(AgentError::Model(format!("HTTP {code}: {detail}")));
                }
                Err(err) => {
                    if attempt < self.max_retries {
                        std::thread::sleep(Duration::from_secs(2u64.pow(attempt)));
                        attempt += 1;
                        continue;
                    }
                    return Err(AgentError::Model(format!("transport: {err}")));
                }
            }
        }
    }
}

/// Locates the `music-server` binary: next to the current executable, or on PATH.
///
/// # Errors
/// Fails when the binary cannot be found.
pub fn find_server_binary() -> Result<PathBuf, ProviderError> {
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name(server_name());
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    // Fall back to PATH lookup.
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(server_name());
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(ProviderError::ServerBinaryNotFound)
}

fn server_name() -> &'static str {
    if cfg!(windows) {
        "music-server.exe"
    } else {
        "music-server"
    }
}

/// Checks a path exists and is a bundle directory (delegates real validation
/// to the server; this is a fast pre-flight for friendlier CLI errors).
pub fn looks_like_bundle(path: &Path) -> bool {
    path.join("project.json").is_file()
}

/// Errors from provider selection and execution.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderError {
    /// No provider is available on this machine.
    #[error(
        "no AI provider available — install Claude Code (subscription mode) or set \
         ANTHROPIC_API_KEY (api mode); choose explicitly with --provider"
    )]
    NoneAvailable,
    /// Unknown provider name.
    #[error("unknown provider '{0}' (expected: subscription | api | auto)")]
    Unknown(String),
    /// API mode requested without credentials.
    #[error("api mode needs ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN in the environment")]
    NoApiCredentials,
    /// The `music-server` binary could not be located.
    #[error("music-server binary not found next to the CLI or on PATH — build with `cargo build`")]
    ServerBinaryNotFound,
    /// Claude Code exited with a failure status.
    #[error("claude exited with status {0}")]
    ClaudeExit(i32),
    /// Filesystem or spawn failure.
    #[error("{0}")]
    Io(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_resolution_prefers_explicit_choice() {
        assert_eq!(
            Provider::resolve(Some("subscription")).unwrap(),
            Provider::Subscription
        );
        assert_eq!(
            Provider::resolve(Some("sub")).unwrap(),
            Provider::Subscription
        );
        assert_eq!(Provider::resolve(Some("api")).unwrap(), Provider::Api);
        assert!(matches!(
            Provider::resolve(Some("gemini")),
            Err(ProviderError::Unknown(_))
        ));
    }
}
