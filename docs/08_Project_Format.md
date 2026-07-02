# 08 — Project Format

Status: Draft v1 · Depends on: `03` · Key ADRs: ADR-0012 (bundle dir + JSON canon + command log)

## 1. Requirements on the Format

From `00`/`01`: open & documented (no lock-in), deterministic replay, undo across sessions,
git-friendly (P3/P4 personas), streamable summaries for AI, large-binary asset handling,
forward/backward version tolerance, and multi-client concurrent access (desktop + MCP).

## 2. Layout (ADR-0012): a **bundle directory**

```
MySong.musicos/
├── project.json           # canonical materialized state (03 §3), pretty-printed, key-sorted
├── log/
│   └── 000001.jsonl …     # append-only command+event log, segmented, checksummed
├── ai/
│   ├── runs/…             # AiRunRecords (06 §6): model ids, prompts, tool calls, seeds
│   └── sessions/…         # provider session snapshots (resumable conversations)
├── assets/
│   ├── objects/ab/cdef…   # content-addressed blobs (samples, renders) — BLAKE3
│   └── manifest.json      # logical name → hash, media metadata
├── cache/                 # waveforms, analysis, compiled graphs — REGENERABLE, gitignored
├── renders/               # exported audio (by convention; content-addressed too)
└── musicos.toml           # format_version, minimum reader version, project-level config
```

### Why a directory bundle (vs single binary file, vs zip, vs SQLite-only)

| Option | Verdict |
|---|---|
| **Directory bundle** ✅ | Diffable, partially loadable, git/LFS-native, robust to partial corruption, macOS treats as package |
| Single SQLite file | Great transactional story, but opaque to git/diff/AI, all-or-nothing corruption; **kept as the *index/cache* layer** (below), not the source of truth |
| Zip container (Ableton-style) | Atomicity nice, but rewrite-on-save punishes large projects; no partial diff |
| Custom binary | Maximum lock-in, exactly what `00` forbids |

A SQLite database lives in `cache/index.db` for fast queries (asset search, log indexing)
but is always rebuildable from the JSON/JSONL truth. Truth is text; speed is cache.

## 3. Canonical State: `project.json`

- Serde-serialized `ProjectState`, **canonical form**: sorted keys, stable array ordering
  (by id), fixed float formatting (floats appear only in mix/param values; musical time is
  integer ticks, ADR-0004) → *byte-stable output for unchanged state*, which makes git
  diffs meaningful and hashes reproducible (NFR-4).
- Human-inspectable by design: a producer can read what the AI did in a PR-style diff.
- Size analysis: symbolic state even for large projects is single-digit MB of JSON;
  compresses ~10× in git. If profiling ever disproves this, the escape hatch is sharding
  (`tracks/*.json`) — format_version bump, same model.

## 4. The Log

- JSONL records: `{ seq, timestamp, actor: user|agent(role)|client_id, txn_id, command,
  events, inverse }`, BLAKE3-chained (each record carries previous hash) — tamper-evident
  history and cheap integrity verification.
- Segmented (`log/000001.jsonl`, 10k records/segment); snapshot+truncate compaction keeps
  bounded replay time: open = load `project.json` (last snapshot) + replay tail.
- Powers: cross-session undo, `music log`/blame ("which agent set that EQ?"), replay
  determinism (`06` §6), and the future collaboration substrate (`02` §8).

## 5. Versioning & Migration

- `format_version` (semver) in `musicos.toml` + per-file schema markers.
- **Readers:** newer readers migrate older projects via a chain of pure
  `migrate_vN_to_vN+1(Value) -> Value` functions (tested against a corpus of fixture
  projects per released version — the corpus is a repo asset from day one).
- **Forward tolerance:** unknown enum variants/fields are preserved via captured raw JSON
  (`#[serde(other)]` + side-channel retention) so an older MusicOS opening a newer project
  degrades gracefully (unknown device kinds render as passthrough with a warning) instead
  of destroying data on save. This rule is a hard review gate for all model changes.
- Writers always write current version; `music migrate --to` exists for explicit pinning.

## 6. Assets

- Content-addressed (BLAKE3) blob store: dedup across copies, integrity by construction,
  rename-proof references (`AssetRef = hash + logical name`).
- Large-file guidance: git-LFS patterns shipped in `music init`'s generated `.gitattributes`.
- Missing-asset policy: project opens with placeholder silence + explicit `MissingAsset`
  diagnostics (never refuse to open — Ardour lesson).

## 7. Concurrency & Atomicity

- All writes atomic: write temp + fsync + rename (per file); log appends fsync'd per txn.
- Single-writer/multi-reader enforced by an advisory lock file with owner metadata; the
  desktop app and MCP server share one writing service in-process — a *second process*
  attaching gets read-only mode with change notifications (matches S2 without inventing
  distributed writes in v1).

## 8. Interop & Export

The bundle is *ours*; interchange is explicit: SMF/MusicXML per `05` §5, stems/audio per
`04` §7, and `music export --archive` producing a self-contained `.musicos.zip` (bundle +
assets, for sharing). DAW-bridge exports (FL Studio, Ableton) are future adapter crates
reading the same `ProjectState`.

## 9. Testing

- Round-trip property tests: `load(save(state)) == state`; canonical-form stability tests
  (serialize twice → identical bytes).
- Corpus tests: every released format version's fixtures must open forever (CI).
- Crash-safety test harness: kill -9 during save/log-append; reopen must succeed with at
  most the in-flight transaction lost.
- Fuzz `project.json`/log parsers (untrusted input — projects get shared).
