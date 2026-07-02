//! Stable Rust façade over MusicOS services.
//!
//! External consumers depend on this crate only; internal crates may be
//! reorganized freely as long as these re-exports hold. The semver promise of
//! the project lives here (`docs/02_System_Architecture.md` §3).

pub use musicos_core_types::{ClipId, ProjectId, Seed, Tempo, Tick, TrackId, PPQ};
pub use musicos_project_service::{ProjectSession, Transaction};
