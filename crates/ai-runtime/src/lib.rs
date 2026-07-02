//! Agent orchestration: sessions, plans, budgets, and provider ports.
//!
//! The prime directive holds here (`docs/06` §1, ADR-0010): the model proposes
//! tool calls; the registry validates and executes them. A hallucinated call is
//! a rejected command, never corrupted state, and every action lands in the
//! project's undoable log.
//!
//! This crate owns the provider-agnostic agent loop for **API mode**: send the
//! registry's tool specs to a [`ModelBackend`], execute returned `tool_use`
//! blocks, feed `tool_result`s back, repeat until the model stops. Budgets cap
//! the loop (`docs/06` §4). The backend port keeps the loop offline-testable
//! (`MockBackend` in tests) and provider-swappable; concrete backends live in
//! `musicos-ai-providers`. Subscription mode needs no loop on our side — the
//! Claude Code CLI runs it against our MCP server (`docs/06` §3).

use musicos_tools::{ProjectCtx, Registry};
use serde_json::{json, Value};

/// One request/response exchange with a language model.
///
/// The request/response shapes are Anthropic Messages API JSON; backends for
/// other providers translate at this boundary.
pub trait ModelBackend {
    /// Sends one Messages-API request body, returning the response body.
    ///
    /// # Errors
    /// Returns [`AgentError::Model`] for transport or API failures. Backends
    /// own retry policy for retryable statuses (429/5xx).
    fn complete(&self, body: &Value) -> Result<Value, AgentError>;
}

/// Budgets and model configuration for one agent run.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Model id (Anthropic naming).
    pub model: String,
    /// Hard cap on model round-trips (docs/06 §4: runaway-agent protection).
    pub max_turns: u32,
    /// Per-response output-token ceiling.
    pub max_tokens: u32,
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            model: "claude-opus-4-8".to_string(),
            max_turns: 16,
            max_tokens: 16_000,
        }
    }
}

/// What an agent run produced.
#[derive(Debug)]
pub struct AgentOutcome {
    /// The model's final text answer.
    pub reply: String,
    /// Number of model round-trips used.
    pub turns: u32,
    /// Number of tool calls executed (including rejected ones).
    pub tool_calls: u32,
    /// True if the run stopped because a budget was exhausted.
    pub budget_exhausted: bool,
}

/// System prompt for the production agent. Stable text first — it is the
/// prompt-cache prefix (docs/06 §5; cache breakpoint set on this block).
const SYSTEM_PROMPT: &str = "You are the MusicOS production agent. You control a real music \
project exclusively through the provided tools; every mutation is validated, logged, and \
undoable. Start with get_project_summary when project state is unknown. Prefer the fewest \
tool calls that accomplish the request; render_song is slow, so call it once, at the end, \
and only if the user asked for audio. If a tool returns an error, adjust and retry rather \
than giving up. When finished, reply with one short sentence per change you made.";

/// Runs the API-mode agent loop: brief in, tools executed, final text out.
///
/// # Errors
/// Returns [`AgentError`] on transport failure, a refusal, or a malformed
/// response. Budget exhaustion is not an error — see
/// [`AgentOutcome::budget_exhausted`].
#[allow(clippy::too_many_lines)] // one arm per stop_reason; split when providers multiply
pub fn run_agent(
    backend: &dyn ModelBackend,
    registry: &Registry,
    ctx: &mut ProjectCtx,
    config: &AgentConfig,
    brief: &str,
) -> Result<AgentOutcome, AgentError> {
    let tools: Vec<Value> = registry
        .specs()
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "description": s.description,
                "input_schema": s.params_schema,
            })
        })
        .collect();

    let mut messages = vec![json!({ "role": "user", "content": brief })];
    let mut turns = 0u32;
    let mut tool_calls = 0u32;
    let mut last_text = String::new();

    loop {
        if turns >= config.max_turns {
            return Ok(AgentOutcome {
                reply: last_text,
                turns,
                tool_calls,
                budget_exhausted: true,
            });
        }
        turns += 1;

        let body = json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            // Stable prefix (tools render first, then system) is cached across
            // loop iterations; the top-level marker caches the growing
            // conversation (shared/prompt-caching rules).
            "system": [{
                "type": "text",
                "text": SYSTEM_PROMPT,
                "cache_control": { "type": "ephemeral" },
            }],
            "thinking": { "type": "adaptive" },
            "cache_control": { "type": "ephemeral" },
            "tools": tools,
            "messages": messages,
        });

        let response = backend.complete(&body)?;
        let stop_reason = response["stop_reason"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let content = response["content"].clone();

        // Collect text (for the final reply) and tool_use blocks.
        let blocks = content.as_array().cloned().unwrap_or_default();
        let text: String = blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if !text.trim().is_empty() {
            last_text = text;
        }

        match stop_reason.as_str() {
            "end_turn" | "stop_sequence" => {
                return Ok(AgentOutcome {
                    reply: last_text,
                    turns,
                    tool_calls,
                    budget_exhausted: false,
                });
            }
            "refusal" => return Err(AgentError::Refusal),
            "max_tokens" => return Err(AgentError::Truncated),
            "pause_turn" => {
                // Server-side pause: echo the assistant turn and continue.
                messages.push(json!({ "role": "assistant", "content": content }));
            }
            "tool_use" => {
                // Echo the assistant content verbatim (thinking blocks included —
                // the API requires them back unchanged), then execute every tool
                // call and return ALL results in ONE user message.
                messages.push(json!({ "role": "assistant", "content": content }));
                let mut results = Vec::new();
                for block in blocks.iter().filter(|b| b["type"] == "tool_use") {
                    let id = block["id"].as_str().unwrap_or_default();
                    let name = block["name"].as_str().unwrap_or_default();
                    let input = block["input"].clone();
                    tool_calls += 1;
                    match registry.call(name, ctx, input) {
                        Ok(out) => results.push(json!({
                            "type": "tool_result",
                            "tool_use_id": id,
                            "content": out.to_string(),
                        })),
                        Err(err) => results.push(json!({
                            "type": "tool_result",
                            "tool_use_id": id,
                            "content": err.to_string(),
                            "is_error": true,
                        })),
                    }
                }
                if results.is_empty() {
                    return Err(AgentError::Protocol(
                        "stop_reason tool_use without tool_use blocks".to_string(),
                    ));
                }
                messages.push(json!({ "role": "user", "content": results }));
            }
            other => {
                return Err(AgentError::Protocol(format!(
                    "unexpected stop_reason '{other}'"
                )));
            }
        }
    }
}

/// Errors from an agent run.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentError {
    /// Transport or API failure from the model backend.
    #[error("model backend: {0}")]
    Model(String),
    /// The model declined the request for safety reasons.
    #[error("the model declined this request (stop_reason: refusal)")]
    Refusal,
    /// The response hit the output-token ceiling.
    #[error("response truncated at max_tokens — raise --max-tokens or simplify the request")]
    Truncated,
    /// The response did not match the Messages API contract.
    #[error("protocol: {0}")]
    Protocol(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicos_core_types::ProjectId;
    use musicos_project_model::ProjectState;
    use musicos_storage::BundleStore;
    use std::cell::RefCell;
    use std::path::PathBuf;

    /// Scripted backend: returns canned responses, records request bodies.
    struct MockBackend {
        responses: RefCell<Vec<Value>>,
        requests: RefCell<Vec<Value>>,
    }

    impl MockBackend {
        fn new(mut responses: Vec<Value>) -> MockBackend {
            responses.reverse();
            MockBackend {
                responses: RefCell::new(responses),
                requests: RefCell::new(Vec::new()),
            }
        }
    }

    impl ModelBackend for MockBackend {
        fn complete(&self, body: &Value) -> Result<Value, AgentError> {
            self.requests.borrow_mut().push(body.clone());
            self.responses
                .borrow_mut()
                .pop()
                .ok_or_else(|| AgentError::Model("mock exhausted".to_string()))
        }
    }

    fn ctx(name: &str) -> (ProjectCtx, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("musicos-ai-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        BundleStore::create(&dir, &ProjectState::new(ProjectId(1), "AI")).unwrap();
        (ProjectCtx::open(&dir, "agent:api").unwrap(), dir)
    }

    #[test]
    fn tool_loop_executes_and_feeds_results_back() {
        let backend = MockBackend::new(vec![
            json!({
                "stop_reason": "tool_use",
                "content": [
                    { "type": "text", "text": "Adding a track." },
                    { "type": "tool_use", "id": "tu_1", "name": "add_track",
                      "input": { "name": "Keys" } },
                ],
            }),
            json!({
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "Added a Keys track." }],
            }),
        ]);
        let (mut c, dir) = ctx("loop");
        let outcome = run_agent(
            &backend,
            &Registry::new(),
            &mut c,
            &AgentConfig::default(),
            "add a keys track",
        )
        .unwrap();

        assert_eq!(outcome.reply, "Added a Keys track.");
        assert_eq!(outcome.turns, 2);
        assert_eq!(outcome.tool_calls, 1);
        assert!(!outcome.budget_exhausted);
        // The tool actually ran against the project.
        assert_eq!(c.state().tracks.len(), 1);
        assert_eq!(c.state().tracks[0].name, "Keys");
        // Second request carried the tool_result in one user message.
        let requests = backend.requests.borrow();
        let last_msg = requests[1]["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(last_msg["role"], "user");
        assert_eq!(last_msg["content"][0]["type"], "tool_result");
        assert_eq!(last_msg["content"][0]["tool_use_id"], "tu_1");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tool_errors_return_as_is_error_results_not_failures() {
        let backend = MockBackend::new(vec![
            json!({
                "stop_reason": "tool_use",
                "content": [{ "type": "tool_use", "id": "tu_1", "name": "remove_track",
                              "input": { "track_id": 99 } }],
            }),
            json!({
                "stop_reason": "end_turn",
                "content": [{ "type": "text", "text": "That track does not exist." }],
            }),
        ]);
        let (mut c, dir) = ctx("err");
        let outcome = run_agent(
            &backend,
            &Registry::new(),
            &mut c,
            &AgentConfig::default(),
            "remove track 99",
        )
        .unwrap();
        assert_eq!(outcome.tool_calls, 1);
        let requests = backend.requests.borrow();
        let result = requests[1]["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(result["content"][0]["is_error"], true);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn budget_caps_the_loop() {
        // Model asks for tools forever; budget must stop it.
        let looping = json!({
            "stop_reason": "tool_use",
            "content": [{ "type": "tool_use", "id": "tu", "name": "get_project_summary",
                          "input": {} }],
        });
        let backend = MockBackend::new(vec![looping.clone(), looping.clone(), looping]);
        let (mut c, dir) = ctx("budget");
        let config = AgentConfig {
            max_turns: 3,
            ..AgentConfig::default()
        };
        let outcome =
            run_agent(&backend, &Registry::new(), &mut c, &config, "loop forever").unwrap();
        assert!(outcome.budget_exhausted);
        assert_eq!(outcome.turns, 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refusal_surfaces_as_error() {
        let backend = MockBackend::new(vec![json!({ "stop_reason": "refusal", "content": [] })]);
        let (mut c, dir) = ctx("refusal");
        let err = run_agent(
            &backend,
            &Registry::new(),
            &mut c,
            &AgentConfig::default(),
            "x",
        )
        .unwrap_err();
        assert!(matches!(err, AgentError::Refusal));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
