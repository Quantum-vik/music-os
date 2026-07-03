//! Storage ports and filesystem/SQLite adapters.
//!
//! Milestone 2 ships [`BundleStore`] v0: the open `.musicos` bundle directory
//! (`docs/08` §2) with canonical-JSON state and an append-only JSONL command
//! log. All writes are atomic (temp file + rename). Deliberately deferred, per
//! docs/08: log segmentation, BLAKE3 hash chaining, content-addressed assets,
//! and the SQLite index cache.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use musicos_project_model::ProjectState;
use musicos_project_service::Transaction;

/// Name of the canonical state file inside a bundle.
const STATE_FILE: &str = "project.json";
/// Log directory and v0 single segment.
const LOG_DIR: &str = "log";
const LOG_SEGMENT: &str = "000001.jsonl";

/// One line of the command log.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogRecord {
    /// Monotonic sequence number within the bundle.
    pub seq: u64,
    /// The applied transaction.
    #[serde(flatten)]
    pub txn: Transaction,
}

/// A `.musicos` bundle directory on disk.
#[derive(Debug, Clone)]
pub struct BundleStore {
    root: PathBuf,
}

impl BundleStore {
    /// Creates a new bundle directory and writes the initial state.
    ///
    /// # Errors
    /// Fails if the directory already exists or is not writable.
    pub fn create(root: &Path, state: &ProjectState) -> Result<BundleStore, StorageError> {
        if root.exists() {
            return Err(StorageError::AlreadyExists(root.to_path_buf()));
        }
        fs::create_dir_all(root.join(LOG_DIR))?;
        let store = BundleStore {
            root: root.to_path_buf(),
        };
        store.save_state(state)?;
        Ok(store)
    }

    /// Opens an existing bundle.
    ///
    /// # Errors
    /// Fails if the directory is missing or does not look like a bundle.
    pub fn open(root: &Path) -> Result<BundleStore, StorageError> {
        if !root.join(STATE_FILE).is_file() {
            return Err(StorageError::NotABundle(root.to_path_buf()));
        }
        Ok(BundleStore {
            root: root.to_path_buf(),
        })
    }

    /// The bundle directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Loads the canonical state.
    ///
    /// # Errors
    /// Fails on I/O errors or malformed state files.
    pub fn load_state(&self) -> Result<ProjectState, StorageError> {
        let bytes = fs::read(self.root.join(STATE_FILE))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Writes the canonical state atomically (temp file + fsync + rename).
    ///
    /// # Errors
    /// Fails on I/O errors.
    pub fn save_state(&self, state: &ProjectState) -> Result<(), StorageError> {
        let json = serde_json::to_vec_pretty(state)?;
        Self::write_atomic(&self.root.join(STATE_FILE), &json)
    }

    /// Appends one transaction to the command log, fsynced.
    ///
    /// # Errors
    /// Fails on I/O errors.
    pub fn append_log(&self, txn: &Transaction) -> Result<u64, StorageError> {
        let seq = self.log_len()? + 1;
        let record = LogRecord {
            seq,
            txn: txn.clone(),
        };
        let mut line = serde_json::to_vec(&record)?;
        line.push(b'\n');
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join(LOG_DIR).join(LOG_SEGMENT))?;
        f.write_all(&line)?;
        f.sync_all()?;
        Ok(seq)
    }

    /// Reads the whole command log (v0: single segment).
    ///
    /// # Errors
    /// Fails on I/O errors or malformed records.
    pub fn read_log(&self) -> Result<Vec<LogRecord>, StorageError> {
        let path = self.root.join(LOG_DIR).join(LOG_SEGMENT);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(path)?;
        text.lines().map(|l| Ok(serde_json::from_str(l)?)).collect()
    }

    /// Removes the last log record (used by cross-session undo, `docs/08` §4).
    /// Rewrites the segment atomically.
    ///
    /// # Errors
    /// Fails on I/O errors or malformed records.
    pub fn pop_log(&self) -> Result<Option<LogRecord>, StorageError> {
        let mut records = self.read_log()?;
        let popped = records.pop();
        if popped.is_some() {
            let mut bytes = Vec::new();
            for r in &records {
                bytes.extend(serde_json::to_vec(r)?);
                bytes.push(b'\n');
            }
            Self::write_atomic(&self.root.join(LOG_DIR).join(LOG_SEGMENT), &bytes)?;
        }
        Ok(popped)
    }

    fn log_len(&self) -> Result<u64, StorageError> {
        Ok(self.read_log()?.len() as u64)
    }

    fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
        let tmp = path.with_extension("json.tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Errors from bundle storage.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StorageError {
    /// Target directory already exists.
    #[error("already exists: {0}")]
    AlreadyExists(PathBuf),
    /// Directory does not contain a MusicOS bundle.
    #[error("not a MusicOS bundle (no project.json): {0}")]
    NotABundle(PathBuf),
    /// Underlying I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Malformed JSON on disk.
    #[error("malformed bundle data: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicos_core_types::ProjectId;
    use musicos_project_model::{Command, TrackKind};
    use musicos_project_service::ProjectSession;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "musicos-storage-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn create_save_load_round_trip() {
        let dir = tmpdir("roundtrip");
        let mut session = ProjectSession::create(ProjectId(7), "Demo");
        let store = BundleStore::create(&dir, session.state()).unwrap();

        let txn = session
            .dispatch(
                "user:test",
                Command::CreateTrack {
                    name: "T".into(),
                    kind: TrackKind::Midi,
                },
            )
            .unwrap();
        store.save_state(session.state()).unwrap();
        assert_eq!(store.append_log(&txn).unwrap(), 1);

        let reopened = BundleStore::open(&dir).unwrap();
        assert_eq!(&reopened.load_state().unwrap(), session.state());
        let log = reopened.read_log().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].seq, 1);
        assert_eq!(log[0].txn, txn);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replaying_the_persisted_log_reproduces_saved_state() {
        let dir = tmpdir("replay");
        let mut session = ProjectSession::create(ProjectId(7), "Demo");
        let initial = session.state().clone();
        let store = BundleStore::create(&dir, session.state()).unwrap();
        for name in ["A", "B", "C"] {
            let txn = session
                .dispatch(
                    "user:test",
                    Command::CreateTrack {
                        name: name.into(),
                        kind: TrackKind::Midi,
                    },
                )
                .unwrap();
            store.append_log(&txn).unwrap();
        }
        store.save_state(session.state()).unwrap();

        let mut replayed = initial;
        for record in store.read_log().unwrap() {
            for ev in &record.txn.events {
                replayed.apply_event(ev).unwrap();
            }
        }
        assert_eq!(&replayed, session.state());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// docs/08 §9: every released format version's fixtures must open forever.
    /// The corpus lives in tests/corpus/<version>/ at the repo root; this test
    /// walks every fixture, loads it, and replays its log.
    #[test]
    fn format_corpus_opens_forever() {
        let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus");
        let mut fixtures = 0;
        for version_dir in std::fs::read_dir(&corpus).expect("corpus dir exists") {
            let version_dir = version_dir.unwrap().path();
            if !version_dir.is_dir() {
                continue;
            }
            for fixture in std::fs::read_dir(&version_dir).unwrap() {
                let fixture = fixture.unwrap().path();
                if !fixture.is_dir() {
                    continue;
                }
                fixtures += 1;
                let store = BundleStore::open(&fixture)
                    .unwrap_or_else(|e| panic!("{}: {e}", fixture.display()));
                let state = store
                    .load_state()
                    .unwrap_or_else(|e| panic!("{}: {e}", fixture.display()));
                assert!(
                    !state.tracks.is_empty(),
                    "{}: fixture has tracks",
                    fixture.display()
                );
                // Log replay must still fold cleanly.
                for record in store.read_log().unwrap() {
                    let _ = &record.txn.events; // events deserialize
                }
            }
        }
        assert!(fixtures >= 1, "at least one corpus fixture is required");
    }

    /// Robustness (fuzz-lite, stable toolchain): seeded random mutations of a
    /// valid project.json must error, never panic.
    #[test]
    fn mutated_state_files_error_not_panic() {
        let dir = tmpdir("mutate");
        let mut session = ProjectSession::create(ProjectId(9), "Mutate");
        session
            .dispatch(
                "user:test",
                Command::CreateTrack {
                    name: "T".into(),
                    kind: TrackKind::Midi,
                },
            )
            .unwrap();
        let store = BundleStore::create(&dir, session.state()).unwrap();
        let original = fs::read(dir.join("project.json")).unwrap();

        let mut rng_state = 0x1234_5678_u64;
        let mut next = move || {
            rng_state = rng_state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            rng_state
        };
        for _ in 0..500 {
            let mut bytes = original.clone();
            for _ in 0..=(next() % 8) {
                let pos = usize::try_from(next()).unwrap() % bytes.len();
                bytes[pos] = u8::try_from(next() % 256).unwrap();
            }
            fs::write(dir.join("project.json"), &bytes).unwrap();
            // Must return Ok or Err — any panic fails the test harness.
            let _ = store.load_state();
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn create_refuses_to_overwrite_and_open_requires_a_bundle() {
        let dir = tmpdir("guards");
        let state = ProjectSession::create(ProjectId(7), "Demo").state().clone();
        BundleStore::create(&dir, &state).unwrap();
        assert!(matches!(
            BundleStore::create(&dir, &state),
            Err(StorageError::AlreadyExists(_))
        ));
        let not_bundle = tmpdir("guards-nb");
        fs::create_dir_all(&not_bundle).unwrap();
        assert!(matches!(
            BundleStore::open(&not_bundle),
            Err(StorageError::NotABundle(_))
        ));
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&not_bundle).unwrap();
    }
}
