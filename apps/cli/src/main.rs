//! MusicOS command-line client (`music`).
//!
//! Phase 0: proves the client → service → domain layering with one command.
//! The real CLI (clap-derived from the tool registry, `--json` everywhere) is
//! specified in `docs/02_System_Architecture.md` §4 and lands in Phase 1.

use std::process::ExitCode;

use musicos_project_service::ProjectService;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some("init"), Some(name)) = (args.next().as_deref(), args.next()) else {
        eprintln!("MusicOS (Phase 0 scaffold)\n\nusage: music init <name>");
        return ExitCode::FAILURE;
    };
    let mut service = ProjectService::new();
    match service.create_project(&name) {
        Ok(info) => {
            println!("created project '{}' (id {})", info.name, info.id.0);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
