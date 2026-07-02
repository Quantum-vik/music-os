//! Model Context Protocol server over the tool registry.
//!
//! Exposes every registry tool (`docs/02` §4) as an MCP tool over a
//! newline-delimited JSON-RPC 2.0 stdio transport — the transport Claude Code
//! spawns MCP servers with. Because the host (Claude Code, Claude Desktop, any
//! MCP client) supplies the model, MusicOS needs **no API keys**: the user's
//! subscription does the reasoning; MusicOS executes deterministically
//! (`docs/06` §1, `docs/07`).
//!
//! Token-efficiency by construction: terse tool descriptions, strict input
//! schemas, and `{ data, summary }` outputs where the summary line is what a
//! model needs to continue (`docs/07` §3).
//!
//! v0 deviation from ADR-0011, recorded in `docs/adr/README.md`: this is a
//! minimal hand-rolled implementation of the tools surface (initialize,
//! tools/list, tools/call, ping) with zero new dependencies. The `rmcp` SDK
//! replaces it when resources, HTTP transport, or auth land; the registry
//! keeps that swap contained to this crate.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use musicos_core_types::ProjectId;
use musicos_project_model::ProjectState;
use musicos_storage::BundleStore;
use musicos_tools::{ProjectCtx, Registry, ToolError};
use serde_json::{json, Value};

/// MCP protocol revision this server reports.
const PROTOCOL_VERSION: &str = "2024-11-05";
/// Actor recorded in the project log for MCP-driven commands.
const ACTOR: &str = "agent:mcp";

/// Serves MCP over the given transport until EOF. Generic over I/O for tests;
/// the `music-server` binary passes stdin/stdout.
///
/// # Errors
/// Returns transport-level I/O errors only; protocol and tool errors are
/// reported in-band as JSON-RPC errors / tool results.
pub fn serve(
    input: impl BufRead,
    mut output: impl Write,
    project: Option<PathBuf>,
) -> std::io::Result<()> {
    let mut server = McpServer {
        registry: Registry::new(),
        project,
    };
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Some(response) = server.handle_line(&line) else {
            continue; // notification: no response
        };
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}

struct McpServer {
    registry: Registry,
    project: Option<PathBuf>,
}

impl McpServer {
    fn handle_line(&mut self, line: &str) -> Option<Value> {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(rpc_error(
                    &Value::Null,
                    -32700,
                    &format!("parse error: {e}"),
                ))
            }
        };
        let id = msg.get("id").cloned();
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));

        // Notifications (no id) get no response.
        let id = match id {
            Some(id) if !id.is_null() => id,
            _ => return None,
        };

        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or(PROTOCOL_VERSION),
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "musicos",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "instructions": "MusicOS: deterministic music production tools. \
                    Start with get_project_summary (cheap) to see project state; \
                    create_project if none exists. Every mutation is undoable.",
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(self.list_tools()),
            "tools/call" => Ok(self.call_tool(&params)),
            _ => Err((-32601, format!("method not found: {method}"))),
        };

        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => rpc_error(&id, code, &message),
        })
    }

    fn list_tools(&self) -> Value {
        let mut tools: Vec<Value> = self
            .registry
            .specs()
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "description": s.description,
                    "inputSchema": s.params_schema,
                })
            })
            .collect();
        tools.push(json!({
            "name": "create_project",
            "description": "Create a new MusicOS project bundle at a directory path \
                            (e.g. 'MySong.musicos') and make it the active project.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Bundle directory to create" },
                    "name": { "type": "string", "description": "Project name" }
                },
                "required": ["path", "name"]
            },
        }));
        json!({ "tools": tools })
    }

    /// Tool errors are reported as successful JSON-RPC responses with
    /// `isError: true`, per the MCP spec — the model can read and react.
    fn call_tool(&mut self, params: &Value) -> Value {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        match self.dispatch(name, args) {
            Ok(out) => {
                // Summary first (what the model usually needs), compact JSON after.
                let summary = out
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let text = if summary.is_empty() {
                    out.to_string()
                } else {
                    format!("{summary}\n{out}")
                };
                json!({ "content": [{ "type": "text", "text": text }], "isError": false })
            }
            Err(err) => json!({
                "content": [{ "type": "text", "text": format!("{err}") }],
                "isError": true,
            }),
        }
    }

    fn dispatch(&mut self, name: &str, args: Value) -> Result<Value, ToolError> {
        if name == "create_project" {
            return self.create_project(&args);
        }
        let path = self.resolve_project()?;
        let mut ctx = ProjectCtx::open(&path, ACTOR)?;
        self.registry.call(name, &mut ctx, args)
    }

    fn create_project(&mut self, args: &Value) -> Result<Value, ToolError> {
        let path = args.get("path").and_then(Value::as_str).ok_or(ToolError {
            code: "E_INVALID_INPUT",
            message: "'path' is required".to_string(),
        })?;
        let name = args.get("name").and_then(Value::as_str).ok_or(ToolError {
            code: "E_INVALID_INPUT",
            message: "'name' is required".to_string(),
        })?;
        let id = ProjectId(name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3)
        }));
        BundleStore::create(Path::new(path), &ProjectState::new(id, name)).map_err(|e| {
            ToolError {
                code: "E_STORAGE",
                message: e.to_string(),
            }
        })?;
        self.project = Some(PathBuf::from(path));
        Ok(json!({
            "path": path,
            "summary": format!("created project '{name}' at {path}; it is now active"),
        }))
    }

    /// Active project: explicit flag, else the single `*.musicos` in cwd.
    fn resolve_project(&self) -> Result<PathBuf, ToolError> {
        if let Some(p) = &self.project {
            return Ok(p.clone());
        }
        let bundles: Vec<PathBuf> = std::fs::read_dir(".")
            .map_err(|e| ToolError {
                code: "E_STORAGE",
                message: e.to_string(),
            })?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.extension().is_some_and(|e| e == "musicos"))
            .collect();
        match bundles.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(ToolError {
                code: "E_NO_PROJECT",
                message: "no .musicos project found — call create_project first".to_string(),
            }),
            _ => Err(ToolError {
                code: "E_AMBIGUOUS_PROJECT",
                message: "multiple .musicos projects in cwd — start the server with --project"
                    .to_string(),
            }),
        }
    }
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(lines: &[Value], project: Option<PathBuf>) -> Vec<Value> {
        let input: String = lines.iter().fold(String::new(), |mut s, l| {
            use std::fmt::Write as _;
            let _ = writeln!(s, "{l}");
            s
        });
        let mut out = Vec::new();
        serve(input.as_bytes(), &mut out, project).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("musicos-mcp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn full_conversation_against_a_fresh_project() {
        let dir = tmp("conv");
        let responses = drive(
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
                json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"create_project","arguments":{"path":dir.display().to_string(),"name":"McpSong"}}}),
                json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"add_track","arguments":{"name":"Keys"}}}),
                json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_project_summary","arguments":{}}}),
                json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"remove_track","arguments":{"track_id":99}}}),
            ],
            None,
        );
        // initialize
        assert_eq!(responses[0]["result"]["serverInfo"]["name"], "musicos");
        // tools/list contains registry + create_project
        let tools = responses[1]["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|t| t["name"] == "render_song"));
        assert!(tools.iter().any(|t| t["name"] == "create_project"));
        // create + add_track + summary
        assert_eq!(responses[2]["result"]["isError"], false);
        assert_eq!(responses[3]["result"]["isError"], false);
        let text = responses[4]["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(
            text.contains("McpSong") && text.contains("1 track(s)"),
            "{text}"
        );
        // domain error surfaces as isError, not a protocol failure
        assert_eq!(responses[5]["result"]["isError"], true);
        assert!(responses[5]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("E_DOMAIN"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn notifications_get_no_response_and_unknown_methods_error() {
        let responses = drive(
            &[
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                json!({"jsonrpc":"2.0","id":7,"method":"bogus/method"}),
            ],
            None,
        );
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["error"]["code"], -32601);
    }

    #[test]
    fn missing_project_yields_actionable_tool_error() {
        let dir = tmp("empty-cwd"); // points --project at a non-bundle
        let responses = drive(
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_project_summary","arguments":{}}}),
            ],
            Some(dir),
        );
        assert_eq!(responses[0]["result"]["isError"], true);
    }
}
