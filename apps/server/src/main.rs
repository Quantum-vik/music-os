//! MusicOS MCP server and API host.
//!
//! Phase 0 placeholder binary. The MCP surface (rmcp over the tool registry,
//! stdio + HTTP transports) is specified in `docs/07_MCP_Architecture.md` and
//! lands in Phase 3.

use std::process::ExitCode;

use musicos_core_types::ProjectId;
use musicos_project_service::ProjectSession;

fn main() -> ExitCode {
    // Touch the service layer so the layering is exercised from every app.
    let _ = ProjectSession::create(ProjectId(0), "placeholder");
    eprintln!("music-server: MCP server lands in Phase 3 (docs/12_Development_Roadmap.md)");
    ExitCode::FAILURE
}
