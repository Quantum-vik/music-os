//! Application service for project command handling, undo, and snapshots.
//!
//! Phase 0: a deliberately trivial service proving the layering
//! (`apps/* → project-service → core-types`). The real command/actor design is
//! specified in `docs/02_System_Architecture.md` §6 and `docs/10_Thread_Model.md` §2
//! and lands in Phase 1.

use musicos_core_types::ProjectId;

/// Summary of a project as reported by the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    /// Stable identifier for the project.
    pub id: ProjectId,
    /// Human-readable project name.
    pub name: String,
}

/// Entry point for project operations.
///
/// Phase 0 stub: in-memory, no persistence, no commands. Exists so clients
/// exercise the dependency rule from day one.
#[derive(Debug, Default)]
pub struct ProjectService {
    next_id: u64,
}

impl ProjectService {
    /// Creates a new, empty service.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a project and returns its info.
    ///
    /// # Errors
    /// Returns [`ProjectError::EmptyName`] if `name` is blank.
    pub fn create_project(&mut self, name: &str) -> Result<ProjectInfo, ProjectError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ProjectError::EmptyName);
        }
        let id = ProjectId(self.next_id);
        self.next_id += 1;
        Ok(ProjectInfo {
            id,
            name: name.to_string(),
        })
    }
}

/// Errors returned by [`ProjectService`].
#[derive(Debug, PartialEq, Eq)]
pub enum ProjectError {
    /// The provided project name was empty or whitespace-only.
    EmptyName,
}

impl core::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProjectError::EmptyName => write!(f, "project name must not be empty"),
        }
    }
}

impl std::error::Error for ProjectError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_projects_with_unique_ids() {
        let mut svc = ProjectService::new();
        let a = svc.create_project("Alpha").unwrap();
        let b = svc.create_project("Beta").unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(a.name, "Alpha");
    }

    #[test]
    fn rejects_blank_names() {
        let mut svc = ProjectService::new();
        assert_eq!(svc.create_project("   "), Err(ProjectError::EmptyName));
    }
}
