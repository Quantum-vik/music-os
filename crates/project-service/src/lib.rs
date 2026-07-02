//! Application service for project command handling, undo, and snapshots.
//!
//! [`ProjectSession`] is the single writer for a project (`docs/02` §6): it
//! validates commands through the aggregate, groups the resulting events into
//! transactions, and maintains undo/redo stacks of those transactions. The
//! async `ProjectActor` wrapper arrives with the service runtime in Phase 3;
//! the session is deliberately sync so it stays trivially testable.

use musicos_core_types::ProjectId;
use musicos_project_model::{Command, DomainError, Event, ProjectState};

/// One applied transaction: the command and the events it produced.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Transaction {
    /// Who issued the command (`user:cli`, `agent:composer`, …).
    pub actor: String,
    /// The validated command.
    pub command: Command,
    /// The events that were applied.
    pub events: Vec<Event>,
}

/// A live editing session over one project. Single writer by construction.
#[derive(Debug)]
pub struct ProjectSession {
    state: ProjectState,
    undo: Vec<Vec<Event>>,
    redo: Vec<Vec<Event>>,
}

impl ProjectSession {
    /// Starts a session over a fresh project.
    pub fn create(id: ProjectId, name: &str) -> ProjectSession {
        ProjectSession::from_state(ProjectState::new(id, name))
    }

    /// Starts a session over loaded state (undo history starts empty:
    /// cross-session undo replays the persisted log instead — `docs/08` §4).
    pub fn from_state(state: ProjectState) -> ProjectSession {
        ProjectSession {
            state,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// Read access to the current state (cheap; clone if you need a snapshot).
    pub fn state(&self) -> &ProjectState {
        &self.state
    }

    /// Validates and applies a command as one undoable transaction.
    ///
    /// # Errors
    /// Returns [`DomainError`] and changes nothing if validation fails.
    pub fn dispatch(&mut self, actor: &str, command: Command) -> Result<Transaction, DomainError> {
        let events = self.state.dispatch(command.clone())?;
        self.undo.push(events.clone());
        self.redo.clear();
        Ok(Transaction {
            actor: actor.to_string(),
            command,
            events,
        })
    }

    /// Undoes the most recent transaction. Returns the inverse events that
    /// were applied, or `None` if there is nothing to undo.
    ///
    /// # Errors
    /// Returns [`DomainError`] only if state and history have diverged (a bug).
    pub fn undo(&mut self) -> Result<Option<Vec<Event>>, DomainError> {
        let Some(events) = self.undo.pop() else {
            return Ok(None);
        };
        let inverse = Event::inverse_transaction(&events);
        for ev in &inverse {
            self.state.apply_event(ev)?;
        }
        self.redo.push(events);
        Ok(Some(inverse))
    }

    /// Re-applies the most recently undone transaction.
    ///
    /// # Errors
    /// Returns [`DomainError`] only if state and history have diverged (a bug).
    pub fn redo(&mut self) -> Result<Option<Vec<Event>>, DomainError> {
        let Some(events) = self.redo.pop() else {
            return Ok(None);
        };
        for ev in &events {
            self.state.apply_event(ev)?;
        }
        self.undo.push(events.clone());
        Ok(Some(events))
    }

    /// Number of transactions available to undo / redo.
    pub fn history_depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicos_project_model::TrackKind;

    fn track_cmd(name: &str) -> Command {
        Command::CreateTrack {
            name: name.into(),
            kind: TrackKind::Midi,
        }
    }

    #[test]
    fn dispatch_undo_redo_cycle() {
        let mut s = ProjectSession::create(ProjectId(1), "P");
        let empty = s.state().clone();
        s.dispatch("user:test", track_cmd("A")).unwrap();
        let with_track = s.state().clone();

        assert!(s.undo().unwrap().is_some());
        assert_eq!(s.state().tracks.len(), empty.tracks.len());
        assert!(s.redo().unwrap().is_some());
        assert_eq!(s.state(), &with_track);
        assert_eq!(s.history_depth(), (1, 0));
    }

    #[test]
    fn new_dispatch_clears_redo() {
        let mut s = ProjectSession::create(ProjectId(1), "P");
        s.dispatch("user:test", track_cmd("A")).unwrap();
        s.undo().unwrap();
        s.dispatch("user:test", track_cmd("B")).unwrap();
        assert!(s.redo().unwrap().is_none());
        assert_eq!(s.state().tracks.len(), 1);
        assert_eq!(s.state().tracks[0].name, "B");
    }

    #[test]
    fn undo_on_empty_history_is_a_noop() {
        let mut s = ProjectSession::create(ProjectId(1), "P");
        assert!(s.undo().unwrap().is_none());
        assert!(s.redo().unwrap().is_none());
    }

    #[test]
    fn failed_dispatch_leaves_history_untouched() {
        let mut s = ProjectSession::create(ProjectId(1), "P");
        assert!(s.dispatch("user:test", track_cmd("  ")).is_err());
        assert_eq!(s.history_depth(), (0, 0));
    }
}
