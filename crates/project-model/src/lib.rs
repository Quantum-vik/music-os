//! Project aggregate: tracks, clips, automation, commands, and events.
//!
//! The aggregate is mutated **only** through [`ProjectState::dispatch`], which
//! validates a [`Command`] and folds the resulting [`Event`]s into the state
//! (`docs/03` §3–4). Events are past-tense facts carrying enough data to be
//! inverted, which is what gives undo/redo, audit, and replay for free
//! (ADR-0003). Automation lanes and the mix graph land in a later milestone.

use std::collections::BTreeMap;

use musicos_core_types::{ClipId, ProjectId, Tempo, Tick, TrackId};
use musicos_music_core::Pattern;
use musicos_timeline::{SignatureMap, TempoMap};
use serde::{Deserialize, Serialize};

/// Current on-disk format version written by this crate (`docs/08` §5).
pub const FORMAT_VERSION: &str = "0.1.0";

/// Project identity and format metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectMeta {
    /// Stable project identifier.
    pub id: ProjectId,
    /// Human-readable project name.
    pub name: String,
    /// Format version this state was written with.
    pub format_version: String,
}

/// What a track holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackKind {
    /// Symbolic (MIDI) content.
    Midi,
    /// Audio content (clips land in a later milestone).
    Audio,
    /// A mix bus (no clips).
    Bus,
}

/// A clip placed on a track at a timeline position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    /// The placed clip.
    pub clip: ClipId,
    /// Timeline position of the clip start.
    pub at: Tick,
}

/// Per-track mix settings (`docs/03` §3 `ChannelStrip`; inserts and sends land
/// with the bus/mix-graph milestone).
///
/// `#[serde(default)]` keeps bundles written before this field readable —
/// the forward-tolerance rule from `docs/08` §5.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChannelStrip {
    /// Gain in decibels (0.0 = unity). Valid range −96.0..=12.0.
    pub gain_db: f32,
    /// Pan position −1.0 (left) ..= 1.0 (right); equal-power law at render.
    pub pan: f32,
    /// Muted tracks produce silence.
    pub muted: bool,
}

impl Default for ChannelStrip {
    fn default() -> Self {
        ChannelStrip {
            gain_db: 0.0,
            pan: 0.0,
            muted: false,
        }
    }
}

/// A track: name, kind, mix settings, and clip placements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// Stable track identifier.
    pub id: TrackId,
    /// Display name.
    pub name: String,
    /// Content kind.
    pub kind: TrackKind,
    /// Mix settings.
    #[serde(default)]
    pub mix: ChannelStrip,
    /// Clips placed on this track, in insertion order.
    pub placements: Vec<Placement>,
}

/// Clip content (symbolic only in this milestone).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clip {
    /// Display name.
    pub name: String,
    /// The symbolic content.
    pub pattern: Pattern,
}

/// The project aggregate root. See the crate docs for the mutation contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectState {
    /// Identity and format metadata.
    pub meta: ProjectMeta,
    /// Tempo changes (invariant TM1 enforced by `musicos-timeline`).
    pub tempo_map: TempoMap,
    /// Meter changes.
    pub signature_map: SignatureMap,
    /// Tracks in mixer order.
    pub tracks: Vec<Track>,
    /// Clip contents, referenced by placements (invariant ID1: no dangling
    /// references can be constructed through commands).
    pub clips: BTreeMap<ClipId, Clip>,
    next_track: u64,
    next_clip: u64,
}

impl ProjectState {
    /// A new, empty project.
    pub fn new(id: ProjectId, name: &str) -> ProjectState {
        ProjectState {
            meta: ProjectMeta {
                id,
                name: name.to_string(),
                format_version: FORMAT_VERSION.to_string(),
            },
            tempo_map: TempoMap::default(),
            signature_map: SignatureMap::default(),
            tracks: Vec::new(),
            clips: BTreeMap::new(),
            next_track: 0,
            next_clip: 0,
        }
    }

    /// Validates a command and applies its events, returning them.
    ///
    /// This is the **only** mutation entry point; every returned event has
    /// already been folded into the state.
    ///
    /// # Errors
    /// Returns [`DomainError`] and leaves the state untouched if validation
    /// fails.
    pub fn dispatch(&mut self, command: Command) -> Result<Vec<Event>, DomainError> {
        let events = self.plan(command)?;
        for ev in &events {
            self.apply_event(ev)?;
        }
        Ok(events)
    }

    /// Validates a command against the current state and produces its events
    /// without applying them.
    #[allow(clippy::too_many_lines)] // one arm per command; split into per-command fns when it grows
    fn plan(&mut self, command: Command) -> Result<Vec<Event>, DomainError> {
        match command {
            Command::RenameProject { name } => {
                let name = non_empty(&name)?;
                Ok(vec![Event::ProjectRenamed {
                    from: self.meta.name.clone(),
                    to: name,
                }])
            }
            Command::CreateTrack { name, kind } => {
                let name = non_empty(&name)?;
                let id = TrackId(self.next_track);
                self.next_track += 1; // ids are never reissued, even across undo
                Ok(vec![Event::TrackCreated {
                    track: Track {
                        id,
                        name,
                        kind,
                        mix: ChannelStrip::default(),
                        placements: Vec::new(),
                    },
                    index: self.tracks.len(),
                }])
            }
            Command::RenameTrack { track, name } => {
                let name = non_empty(&name)?;
                let t = self.track(track)?;
                Ok(vec![Event::TrackRenamed {
                    track,
                    from: t.name.clone(),
                    to: name,
                }])
            }
            Command::RemoveTrack { track } => {
                let index = self.track_index(track)?;
                let t = self.tracks[index].clone();
                let clips = t
                    .placements
                    .iter()
                    .map(|p| (p.clip, self.clips[&p.clip].clone()))
                    .collect();
                Ok(vec![Event::TrackRemoved {
                    track: t,
                    index,
                    clips,
                }])
            }
            Command::InsertClip {
                track,
                name,
                pattern,
                at,
            } => {
                let name = non_empty(&name)?;
                if at < Tick::ZERO {
                    return Err(DomainError::NegativeTick(at));
                }
                let t = self.track(track)?;
                if t.kind == TrackKind::Bus {
                    return Err(DomainError::BusHoldsNoClips(track));
                }
                let clip_id = ClipId(self.next_clip);
                self.next_clip += 1;
                Ok(vec![Event::ClipInserted {
                    track,
                    clip_id,
                    clip: Clip { name, pattern },
                    at,
                }])
            }
            Command::RemoveClip { clip } => {
                let (track, placement) = self.placement_of(clip)?;
                Ok(vec![Event::ClipRemoved {
                    track,
                    clip_id: clip,
                    clip: self.clips[&clip].clone(),
                    at: placement.at,
                }])
            }
            Command::MoveClip { clip, at } => {
                if at < Tick::ZERO {
                    return Err(DomainError::NegativeTick(at));
                }
                let (track, placement) = self.placement_of(clip)?;
                Ok(vec![Event::ClipMoved {
                    track,
                    clip,
                    from: placement.at,
                    to: at,
                }])
            }
            Command::SetTrackGain { track, gain_db } => {
                if !(-96.0..=12.0).contains(&gain_db) {
                    return Err(DomainError::OutOfRange("gain_db", -96.0, 12.0));
                }
                let t = self.track(track)?;
                Ok(vec![Event::TrackGainSet {
                    track,
                    from: t.mix.gain_db,
                    to: gain_db,
                }])
            }
            Command::SetTrackPan { track, pan } => {
                if !(-1.0..=1.0).contains(&pan) {
                    return Err(DomainError::OutOfRange("pan", -1.0, 1.0));
                }
                let t = self.track(track)?;
                Ok(vec![Event::TrackPanSet {
                    track,
                    from: t.mix.pan,
                    to: pan,
                }])
            }
            Command::SetTrackMute { track, muted } => {
                let t = self.track(track)?;
                Ok(vec![Event::TrackMuteSet {
                    track,
                    from: t.mix.muted,
                    to: muted,
                }])
            }
            Command::SetTempo { at, tempo } => {
                if at < Tick::ZERO {
                    return Err(DomainError::NegativeTick(at));
                }
                let from = self
                    .tempo_map
                    .entries()
                    .iter()
                    .find(|(t, _)| *t == at)
                    .map(|(_, x)| *x);
                Ok(vec![Event::TempoSet {
                    at,
                    from,
                    to: tempo,
                }])
            }
        }
    }

    /// Folds one event into the state. Total for events produced by
    /// [`Self::dispatch`]; returns an error only when replaying a log that
    /// does not match the state (corruption).
    ///
    /// # Errors
    /// Returns [`DomainError`] if the event references unknown entities.
    pub fn apply_event(&mut self, event: &Event) -> Result<(), DomainError> {
        match event {
            Event::ProjectRenamed { to, .. } => {
                self.meta.name.clone_from(to);
            }
            Event::TrackCreated { track, index } => {
                let index = (*index).min(self.tracks.len());
                self.tracks.insert(index, track.clone());
                self.next_track = self.next_track.max(track.id.0 + 1);
            }
            Event::TrackRenamed { track, to, .. } => {
                self.track_mut(*track)?.name.clone_from(to);
            }
            Event::TrackRemoved { track, .. } => {
                let index = self.track_index(track.id)?;
                let removed = self.tracks.remove(index);
                for p in &removed.placements {
                    self.clips.remove(&p.clip);
                }
            }
            Event::ClipInserted {
                track,
                clip_id,
                clip,
                at,
            } => {
                self.clips.insert(*clip_id, clip.clone());
                self.next_clip = self.next_clip.max(clip_id.0 + 1);
                self.track_mut(*track)?.placements.push(Placement {
                    clip: *clip_id,
                    at: *at,
                });
            }
            Event::ClipRemoved { track, clip_id, .. } => {
                let t = self.track_mut(*track)?;
                t.placements.retain(|p| p.clip != *clip_id);
                self.clips.remove(clip_id);
            }
            Event::ClipMoved {
                track, clip, to, ..
            } => {
                let t = self.track_mut(*track)?;
                let p = t
                    .placements
                    .iter_mut()
                    .find(|p| p.clip == *clip)
                    .ok_or(DomainError::UnknownClip(*clip))?;
                p.at = *to;
            }
            Event::TrackGainSet { track, to, .. } => {
                self.track_mut(*track)?.mix.gain_db = *to;
            }
            Event::TrackPanSet { track, to, .. } => {
                self.track_mut(*track)?.mix.pan = *to;
            }
            Event::TrackMuteSet { track, to, .. } => {
                self.track_mut(*track)?.mix.muted = *to;
            }
            Event::TempoSet { at, to, .. } => {
                self.tempo_map
                    .set(*at, *to)
                    .map_err(DomainError::Timeline)?;
            }
            Event::TempoUnset { at, .. } => {
                self.tempo_map.remove(*at).map_err(DomainError::Timeline)?;
            }
        }
        Ok(())
    }

    fn track(&self, id: TrackId) -> Result<&Track, DomainError> {
        self.tracks
            .iter()
            .find(|t| t.id == id)
            .ok_or(DomainError::UnknownTrack(id))
    }

    fn track_mut(&mut self, id: TrackId) -> Result<&mut Track, DomainError> {
        self.tracks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(DomainError::UnknownTrack(id))
    }

    fn track_index(&self, id: TrackId) -> Result<usize, DomainError> {
        self.tracks
            .iter()
            .position(|t| t.id == id)
            .ok_or(DomainError::UnknownTrack(id))
    }

    fn placement_of(&self, clip: ClipId) -> Result<(TrackId, Placement), DomainError> {
        for t in &self.tracks {
            if let Some(p) = t.placements.iter().find(|p| p.clip == clip) {
                return Ok((t.id, *p));
            }
        }
        Err(DomainError::UnknownClip(clip))
    }
}

fn non_empty(name: &str) -> Result<String, DomainError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(DomainError::EmptyName);
    }
    Ok(name.to_string())
}

/// Imperative, validated, rejectable requests to change the project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Command {
    /// Rename the project.
    RenameProject {
        /// New project name.
        name: String,
    },
    /// Create a track at the end of the track list.
    CreateTrack {
        /// Track display name.
        name: String,
        /// Track content kind.
        kind: TrackKind,
    },
    /// Rename a track.
    RenameTrack {
        /// Target track.
        track: TrackId,
        /// New display name.
        name: String,
    },
    /// Remove a track and every clip placed on it.
    RemoveTrack {
        /// Target track.
        track: TrackId,
    },
    /// Insert a new clip on a track.
    InsertClip {
        /// Target track.
        track: TrackId,
        /// Clip display name.
        name: String,
        /// Symbolic content.
        pattern: Pattern,
        /// Timeline position.
        at: Tick,
    },
    /// Remove a clip (and its placement).
    RemoveClip {
        /// Target clip.
        clip: ClipId,
    },
    /// Move a clip to a new timeline position on its track.
    MoveClip {
        /// Target clip.
        clip: ClipId,
        /// New timeline position.
        at: Tick,
    },
    /// Set a track's gain in decibels (−96.0..=12.0).
    SetTrackGain {
        /// Target track.
        track: TrackId,
        /// New gain in dB.
        gain_db: f32,
    },
    /// Set a track's pan position (−1.0..=1.0).
    SetTrackPan {
        /// Target track.
        track: TrackId,
        /// New pan position.
        pan: f32,
    },
    /// Mute or unmute a track.
    SetTrackMute {
        /// Target track.
        track: TrackId,
        /// New mute state.
        muted: bool,
    },
    /// Set (or add) a tempo change at a position.
    SetTempo {
        /// Timeline position of the tempo change.
        at: Tick,
        /// New tempo.
        tempo: Tempo,
    },
}

/// Past-tense facts. Each event carries enough data to compute its inverse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Event {
    /// The project was renamed.
    ProjectRenamed {
        /// Previous name.
        from: String,
        /// New name.
        to: String,
    },
    /// A track was created.
    TrackCreated {
        /// The created track.
        track: Track,
        /// Position in the track list.
        index: usize,
    },
    /// A track was renamed.
    TrackRenamed {
        /// Target track.
        track: TrackId,
        /// Previous name.
        from: String,
        /// New name.
        to: String,
    },
    /// A track was removed, with everything needed to restore it.
    TrackRemoved {
        /// The removed track (including placements).
        track: Track,
        /// Its previous position in the track list.
        index: usize,
        /// The removed clip contents.
        clips: Vec<(ClipId, Clip)>,
    },
    /// A clip was inserted.
    ClipInserted {
        /// Host track.
        track: TrackId,
        /// New clip id.
        clip_id: ClipId,
        /// Clip content.
        clip: Clip,
        /// Timeline position.
        at: Tick,
    },
    /// A clip was removed.
    ClipRemoved {
        /// Host track.
        track: TrackId,
        /// Removed clip id.
        clip_id: ClipId,
        /// Removed clip content (for undo).
        clip: Clip,
        /// Its previous timeline position.
        at: Tick,
    },
    /// A clip moved to a new position.
    ClipMoved {
        /// Host track.
        track: TrackId,
        /// Target clip.
        clip: ClipId,
        /// Previous position.
        from: Tick,
        /// New position.
        to: Tick,
    },
    /// A track's gain changed.
    TrackGainSet {
        /// Target track.
        track: TrackId,
        /// Previous gain (dB).
        from: f32,
        /// New gain (dB).
        to: f32,
    },
    /// A track's pan changed.
    TrackPanSet {
        /// Target track.
        track: TrackId,
        /// Previous pan.
        from: f32,
        /// New pan.
        to: f32,
    },
    /// A track's mute state changed.
    TrackMuteSet {
        /// Target track.
        track: TrackId,
        /// Previous state.
        from: bool,
        /// New state.
        to: bool,
    },
    /// A tempo entry was set (added or changed).
    TempoSet {
        /// Timeline position.
        at: Tick,
        /// Previous tempo at this exact position, if any.
        from: Option<Tempo>,
        /// New tempo.
        to: Tempo,
    },
    /// A tempo entry was removed (only produced as an inverse).
    TempoUnset {
        /// Timeline position.
        at: Tick,
        /// The removed tempo.
        tempo: Tempo,
    },
}

impl Event {
    /// The event that exactly undoes this one.
    pub fn inverse(&self) -> Event {
        match self {
            Event::ProjectRenamed { from, to } => Event::ProjectRenamed {
                from: to.clone(),
                to: from.clone(),
            },
            Event::TrackCreated { track, index } => Event::TrackRemoved {
                track: track.clone(),
                index: *index,
                clips: Vec::new(), // a freshly created track has no clips
            },
            Event::TrackRenamed { track, from, to } => Event::TrackRenamed {
                track: *track,
                from: to.clone(),
                to: from.clone(),
            },
            Event::TrackRemoved { track, index, .. } => {
                // Restore the track shell first; clips are restored by the
                // transaction inverse (see `Transaction::inverse`).
                Event::TrackCreated {
                    track: track.clone(),
                    index: *index,
                }
            }
            Event::ClipInserted {
                track,
                clip_id,
                clip,
                at,
            } => Event::ClipRemoved {
                track: *track,
                clip_id: *clip_id,
                clip: clip.clone(),
                at: *at,
            },
            Event::ClipRemoved {
                track,
                clip_id,
                clip,
                at,
            } => Event::ClipInserted {
                track: *track,
                clip_id: *clip_id,
                clip: clip.clone(),
                at: *at,
            },
            Event::ClipMoved {
                track,
                clip,
                from,
                to,
            } => Event::ClipMoved {
                track: *track,
                clip: *clip,
                from: *to,
                to: *from,
            },
            Event::TrackGainSet { track, from, to } => Event::TrackGainSet {
                track: *track,
                from: *to,
                to: *from,
            },
            Event::TrackPanSet { track, from, to } => Event::TrackPanSet {
                track: *track,
                from: *to,
                to: *from,
            },
            Event::TrackMuteSet { track, from, to } => Event::TrackMuteSet {
                track: *track,
                from: *to,
                to: *from,
            },
            Event::TempoSet { at, from, to } => match from {
                Some(prev) => Event::TempoSet {
                    at: *at,
                    from: Some(*to),
                    to: *prev,
                },
                None => Event::TempoUnset {
                    at: *at,
                    tempo: *to,
                },
            },
            Event::TempoUnset { at, tempo } => Event::TempoSet {
                at: *at,
                from: None,
                to: *tempo,
            },
        }
    }

    /// Inverse of a whole transaction: each event inverted, in reverse order,
    /// with removed-track clip restoration expanded.
    pub fn inverse_transaction(events: &[Event]) -> Vec<Event> {
        let mut out = Vec::new();
        for ev in events.iter().rev() {
            match ev {
                Event::TrackRemoved {
                    track,
                    index,
                    clips,
                } => {
                    // Restore shell without placements, then re-insert clips.
                    let mut shell = track.clone();
                    let placements = std::mem::take(&mut shell.placements);
                    out.push(Event::TrackCreated {
                        track: shell,
                        index: *index,
                    });
                    for p in &placements {
                        let clip = clips
                            .iter()
                            .find(|(id, _)| id == &p.clip)
                            .map(|(_, c)| c.clone())
                            .expect("TrackRemoved carries all its clips");
                        out.push(Event::ClipInserted {
                            track: track.id,
                            clip_id: p.clip,
                            clip,
                            at: p.at,
                        });
                    }
                }
                other => out.push(other.inverse()),
            }
        }
        out
    }
}

/// Errors from command validation or event replay.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum DomainError {
    /// A name was empty or whitespace-only.
    #[error("name must not be empty")]
    EmptyName,
    /// A referenced track does not exist.
    #[error("unknown track {0:?}")]
    UnknownTrack(TrackId),
    /// A referenced clip does not exist.
    #[error("unknown clip {0:?}")]
    UnknownClip(ClipId),
    /// A timeline position was negative.
    #[error("position must not be negative (got {0:?})")]
    NegativeTick(Tick),
    /// Bus tracks hold no clips.
    #[error("bus track {0:?} cannot hold clips")]
    BusHoldsNoClips(TrackId),
    /// A numeric parameter was outside its valid range.
    #[error("{0} must be within {1}..={2}")]
    OutOfRange(&'static str, f32, f32),
    /// A timeline map invariant was violated.
    #[error("timeline: {0}")]
    Timeline(musicos_timeline::TimelineError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicos_core_types::{Pitch, Velocity, PPQ};
    use musicos_music_core::Note;

    fn pattern() -> Pattern {
        Pattern::new(
            vec![Note {
                pitch: Pitch::new(60),
                velocity: Velocity::MF,
                start: Tick::ZERO,
                duration: Tick(PPQ),
            }],
            Tick(PPQ * 4),
        )
        .unwrap()
    }

    fn scripted_commands() -> Vec<Command> {
        vec![
            Command::CreateTrack {
                name: "Drums".into(),
                kind: TrackKind::Midi,
            },
            Command::CreateTrack {
                name: "Bass".into(),
                kind: TrackKind::Midi,
            },
            Command::InsertClip {
                track: TrackId(0),
                name: "beat".into(),
                pattern: pattern(),
                at: Tick::ZERO,
            },
            Command::InsertClip {
                track: TrackId(1),
                name: "groove".into(),
                pattern: pattern(),
                at: Tick(PPQ * 4),
            },
            Command::SetTempo {
                at: Tick::ZERO,
                tempo: Tempo::from_bpm(140.0),
            },
            Command::MoveClip {
                clip: ClipId(0),
                at: Tick(PPQ * 8),
            },
            Command::RenameTrack {
                track: TrackId(1),
                name: "Sub Bass".into(),
            },
            Command::RemoveTrack { track: TrackId(0) },
            Command::RenameProject {
                name: "Banger".into(),
            },
            Command::SetTrackGain {
                track: TrackId(1),
                gain_db: -6.0,
            },
            Command::SetTrackPan {
                track: TrackId(1),
                pan: 0.25,
            },
            Command::SetTrackMute {
                track: TrackId(1),
                muted: true,
            },
        ]
    }

    #[test]
    fn dispatch_validates_before_mutating() {
        let mut s = ProjectState::new(ProjectId(1), "P");
        let before = s.clone();
        assert_eq!(
            s.dispatch(Command::RenameTrack {
                track: TrackId(9),
                name: "x".into()
            }),
            Err(DomainError::UnknownTrack(TrackId(9)))
        );
        assert_eq!(
            s.dispatch(Command::CreateTrack {
                name: "  ".into(),
                kind: TrackKind::Midi
            }),
            Err(DomainError::EmptyName)
        );
        assert_eq!(s, before);
    }

    #[test]
    fn bus_tracks_reject_clips() {
        let mut s = ProjectState::new(ProjectId(1), "P");
        s.dispatch(Command::CreateTrack {
            name: "Bus".into(),
            kind: TrackKind::Bus,
        })
        .unwrap();
        assert_eq!(
            s.dispatch(Command::InsertClip {
                track: TrackId(0),
                name: "c".into(),
                pattern: pattern(),
                at: Tick::ZERO,
            }),
            Err(DomainError::BusHoldsNoClips(TrackId(0)))
        );
    }

    #[test]
    fn undo_of_every_transaction_restores_prior_state() {
        let mut s = ProjectState::new(ProjectId(1), "P");
        let mut snapshots = vec![s.clone()];
        let mut txns = Vec::new();
        for cmd in scripted_commands() {
            txns.push(s.dispatch(cmd).unwrap());
            snapshots.push(s.clone());
        }
        // Undo everything, checking each intermediate state (modulo id counters,
        // which are monotonic by design and never reissued).
        for (txn, expected) in txns.iter().rev().zip(snapshots.iter().rev().skip(1)) {
            for ev in Event::inverse_transaction(txn) {
                s.apply_event(&ev).unwrap();
            }
            let mut normalized = s.clone();
            normalized.next_track = expected.next_track;
            normalized.next_clip = expected.next_clip;
            assert_eq!(&normalized, expected);
        }
    }

    #[test]
    fn replaying_the_event_log_reproduces_the_final_state() {
        let mut live = ProjectState::new(ProjectId(1), "P");
        let mut log: Vec<Event> = Vec::new();
        for cmd in scripted_commands() {
            log.extend(live.dispatch(cmd).unwrap());
        }
        let mut replayed = ProjectState::new(ProjectId(1), "P");
        for ev in &log {
            replayed.apply_event(ev).unwrap();
        }
        assert_eq!(replayed, live);
    }

    #[test]
    fn track_removal_undo_restores_clips() {
        let mut s = ProjectState::new(ProjectId(1), "P");
        s.dispatch(Command::CreateTrack {
            name: "T".into(),
            kind: TrackKind::Midi,
        })
        .unwrap();
        s.dispatch(Command::InsertClip {
            track: TrackId(0),
            name: "c".into(),
            pattern: pattern(),
            at: Tick(7),
        })
        .unwrap();
        let before = s.clone();
        let txn = s
            .dispatch(Command::RemoveTrack { track: TrackId(0) })
            .unwrap();
        assert!(s.clips.is_empty());
        for ev in Event::inverse_transaction(&txn) {
            s.apply_event(&ev).unwrap();
        }
        assert_eq!(s, before);
    }

    #[test]
    fn state_serde_round_trips() {
        let mut s = ProjectState::new(ProjectId(1), "P");
        for cmd in scripted_commands() {
            s.dispatch(cmd).unwrap();
        }
        let json = serde_json::to_string(&s).unwrap();
        let back: ProjectState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
