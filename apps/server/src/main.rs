//! MusicOS MCP server (`music-server`).
//!
//! Speaks MCP over stdio — the transport Claude Code and Claude Desktop use to
//! spawn tool servers. Register it with your Claude subscription (no API keys):
//!
//! ```sh
//! claude mcp add musicos -- music-server --project /path/to/Song.musicos
//! ```
//!
//! Without `--project`, the single `*.musicos` bundle in the working directory
//! is used, or the model is told to call `create_project`. HTTP/WebSocket
//! transport and MCP resources land per `docs/07_MCP_Architecture.md`.

use std::io::{stdin, stdout, BufReader};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut project: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--project" | "-P" => project = args.next().map(PathBuf::from),
            "--help" | "-h" => {
                eprintln!(
                    "MusicOS MCP server (stdio)\n\nusage: music-server [--project <dir.musicos>]"
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}");
                return ExitCode::FAILURE;
            }
        }
    }
    match musicos_mcp_server::serve(BufReader::new(stdin().lock()), stdout().lock(), project) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("music-server: transport error: {err}");
            ExitCode::FAILURE
        }
    }
}
