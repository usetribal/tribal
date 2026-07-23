use std::path::Path;

use git2::Repository;
use lineage_core::{
    files_touched, files_written, normalize_repo_path, turn_indexable_text, turn_salience,
    Conversation, LineageError,
};
use lineage_git::{hydrate_conversation, list_session_ids, read_conversation_stored};
use rusqlite::{params, Connection};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SearchError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("lineage error: {0}")]
    Lineage(#[from] LineageError),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, SearchError>;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHit {
    pub session_id: String,
    pub score: f64,
    pub snippet: String,
}

/// One turn-level FTS match. `body` is the stored enriched turn text (already
/// snippet-capped at extraction time by lineage-core), so hits are
/// self-contained for verbatim evidence.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TurnHit {
    pub turn_id: String,
    pub session_id: String,
    pub salience: f64,
    pub body: String,
    pub snippet: String,
    pub score: f64,
}

/// One indexed turn, as stored: the salience-admitted enriched text and its
/// session. What `get_turn` returns.
#[derive(Debug, Clone)]
pub struct TurnRow {
    pub turn_id: String,
    pub session_id: String,
    pub salience: f64,
    pub body: String,
}

/// One stored chunk embedding: which session and chunk it came from, the
/// anchor turn dense evidence should point at, and the vector. The dense
/// retriever loads all of these and scores them against the query vector
/// (brute-force cosine — see `all_chunk_vectors`).
#[derive(Debug, Clone)]
pub struct ChunkVector {
    pub session_id: String,
    pub chunk_index: i64,
    pub turn_id: String,
    pub vector: Vec<f32>,
}

pub struct LineageIndex {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct IndexSchemaInfo {
    pub has_session_files: bool,
    pub has_index_meta: bool,
    pub generation: i64,
}

/// Read-only schema introspection for diagnostics. `open` applies DDL, so an
/// index written by an older binary would be silently repaired by inspecting
/// through it — this reports the file as it is on disk instead.
pub fn inspect_schema(path: impl AsRef<Path>) -> Result<IndexSchemaInfo> {
    if !path.as_ref().exists() {
        return Ok(IndexSchemaInfo {
            has_session_files: false,
            has_index_meta: false,
            generation: 0,
        });
    }
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let table_exists = |name: &str| -> Result<bool> {
        let count: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    };
    let has_session_files = table_exists("session_files")?;
    let has_index_meta = table_exists("index_meta")?;
    let generation = if has_index_meta {
        conn.query_row(
            "SELECT value FROM index_meta WHERE key = 'corpus_generation'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
    } else {
        0
    };
    Ok(IndexSchemaInfo {
        has_session_files,
        has_index_meta,
        generation,
    })
}

impl LineageIndex {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent).map_err(|e| SearchError::Other(e.to_string()))?;
        }
        let conn = Connection::open(path)?;
        let index = Self { conn };
        index.init_schema()?;
        Ok(index)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                agent TEXT NOT NULL,
                started_at TEXT,
                body TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS turns (
                turn_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                turn_index INTEGER NOT NULL,
                salience_class TEXT NOT NULL,
                salience REAL NOT NULL,
                body TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_turns_session ON turns(session_id);
            CREATE VIRTUAL TABLE IF NOT EXISTS turns_fts USING fts5(
                turn_id UNINDEXED,
                body,
                content='turns',
                content_rowid='rowid',
                tokenize="unicode61 tokenchars '-_'"
            );
            CREATE TRIGGER IF NOT EXISTS turns_ai AFTER INSERT ON turns BEGIN
                INSERT INTO turns_fts(rowid, body) VALUES (new.rowid, new.body);
            END;
            CREATE TRIGGER IF NOT EXISTS turns_ad AFTER DELETE ON turns BEGIN
                INSERT INTO turns_fts(turns_fts, rowid, body) VALUES('delete', old.rowid, old.body);
            END;
            CREATE TRIGGER IF NOT EXISTS turns_au AFTER UPDATE ON turns BEGIN
                INSERT INTO turns_fts(turns_fts, rowid, body) VALUES('delete', old.rowid, old.body);
                INSERT INTO turns_fts(rowid, body) VALUES (new.rowid, new.body);
            END;
            CREATE TABLE IF NOT EXISTS session_files (
                file_path TEXT NOT NULL,
                session_id TEXT NOT NULL,
                wrote INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (file_path, session_id)
            );
            CREATE TABLE IF NOT EXISTS index_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS session_vectors (
                session_id TEXT NOT NULL,
                chunk_index INTEGER NOT NULL,
                turn_id TEXT NOT NULL DEFAULT '',
                dim INTEGER NOT NULL,
                vector BLOB NOT NULL,
                model_version TEXT NOT NULL DEFAULT '',
                PRIMARY KEY (session_id, chunk_index)
            );
            "#,
        )?;
        // The session-body FTS predates turn-grain indexing; dropping it on an
        // old index leaves search empty until the next (auto-)rebuild, which is
        // the established upgrade path (pre-release: rebuild, not migrate).
        self.conn.execute_batch(
            r#"
            DROP TRIGGER IF EXISTS sessions_ai;
            DROP TRIGGER IF EXISTS sessions_ad;
            DROP TRIGGER IF EXISTS sessions_au;
            DROP TABLE IF EXISTS sessions_fts;
            "#,
        )?;
        // Indexes created by binaries predating the read/write distinction
        // lack the column; rows stay wrote=0 until the next (re)index.
        let _ = self.conn.execute(
            "ALTER TABLE session_files ADD COLUMN wrote INTEGER NOT NULL DEFAULT 0",
            [],
        );
        // Vectors from binaries predating incremental embedding lack a version
        // tag; the empty default means they never match a real version and so
        // re-embed on the next pass.
        let _ = self.conn.execute(
            "ALTER TABLE session_vectors ADD COLUMN model_version TEXT NOT NULL DEFAULT ''",
            [],
        );
        // Vectors from binaries predating turn-grain retrieval carry no anchor
        // turn; the empty default is never emitted as evidence and such rows
        // re-embed on the next pass anyway (their model_version is stale).
        let _ = self.conn.execute(
            "ALTER TABLE session_vectors ADD COLUMN turn_id TEXT NOT NULL DEFAULT ''",
            [],
        );
        Ok(())
    }

    pub fn index_conversation(&self, conversation: &Conversation) -> Result<()> {
        // One transaction per conversation: a session is hundreds of turn-row
        // inserts, and autocommit would fsync each one — measured at ~50s wall
        // for a 67-session corpus vs ~1s batched.
        let tx = self.conn.unchecked_transaction()?;
        // The explicit empty body keeps this insert valid on indexes created
        // before the turn pivot, whose sessions.body is NOT NULL without a
        // default; the corpus text lives in `turns` now.
        self.conn.execute(
            "INSERT OR REPLACE INTO sessions (id, agent, started_at, body) VALUES (?1, ?2, ?3, '')",
            params![
                conversation.id.as_str(),
                conversation.agent.as_str(),
                conversation.started_at.to_rfc3339(),
            ],
        )?;

        // The FTS document is the turn, weighted by the v0 salience rules.
        // Zero-weight turns (tool results, pure exploration) are excluded
        // entirely — this is what stops a session's exploratory open from
        // ranking it on incidental matches. Delete-then-insert keeps
        // re-imports idempotent, same as session_files below.
        self.conn.execute(
            "DELETE FROM turns WHERE session_id = ?1",
            params![conversation.id.as_str()],
        )?;
        for (turn_index, turn) in conversation.turns.iter().enumerate() {
            let class = turn_salience(turn);
            if class.weight() == 0.0 {
                continue;
            }
            let body = turn_indexable_text(turn);
            if body.is_empty() {
                continue;
            }
            self.conn.execute(
                "INSERT OR REPLACE INTO turns (turn_id, session_id, turn_index, salience_class, salience, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    turn.id.as_str(),
                    conversation.id.as_str(),
                    turn_index as i64,
                    class.as_str(),
                    f64::from(class.weight()),
                    body,
                ],
            )?;
        }

        // Delete-then-insert keeps re-imports idempotent: the touched-file set
        // is derived wholly from the conversation, so stale rows must not
        // survive a re-index of the same session.
        self.conn.execute(
            "DELETE FROM session_files WHERE session_id = ?1",
            params![conversation.id.as_str()],
        )?;
        let workspace_root = Path::new(&conversation.workspace_root);
        let written: std::collections::HashSet<String> = files_written(conversation)
            .iter()
            .map(|p| normalize_repo_path(p, Some(workspace_root)))
            .collect();
        for path in files_touched(conversation) {
            let normalized = normalize_repo_path(&path, Some(workspace_root));
            if normalized.is_empty() {
                continue;
            }
            let wrote = written.contains(&normalized);
            self.conn.execute(
                "INSERT OR IGNORE INTO session_files (file_path, session_id, wrote) VALUES (?1, ?2, ?3)",
                params![normalized, conversation.id.as_str(), wrote as i64],
            )?;
        }

        // The generation is what lets a derived cache (lineage-retrieval) detect
        // that the session corpus changed without being told about imports.
        self.conn.execute(
            "INSERT INTO index_meta (key, value) VALUES ('corpus_generation', '1')
             ON CONFLICT(key) DO UPDATE SET value = CAST(value AS INTEGER) + 1",
            [],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// How many turn hits back one session-level search result. Session search
    /// folds turn hits to their best turn per session, so it over-fetches
    /// turns to avoid one chatty session crowding others out of the fold.
    const TURNS_PER_SESSION_HIT: usize = 8;

    /// Session-level search, aggregated from turn matches: a session ranks by
    /// its best salience-weighted turn, shown with that turn's snippet. One
    /// code path with `search_turns` — SQLite's FTS auxiliary functions do not
    /// survive subquery aggregation, so the fold happens here.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let turn_hits = self.search_turns(query, limit * Self::TURNS_PER_SESSION_HIT)?;

        let mut seen = std::collections::HashSet::new();
        let mut hits = Vec::new();
        // Turn hits arrive best-first, so the first hit per session is its best.
        for hit in turn_hits {
            if !seen.insert(hit.session_id.clone()) {
                continue;
            }
            hits.push(SearchHit {
                session_id: hit.session_id,
                snippet: hit.snippet,
                score: hit.score,
            });
            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    }

    /// Turn-level search: the intent-retrieval unit. Ranked by
    /// salience-weighted bm25 (negative, lower = better, so multiplying by
    /// salience ∈ (0,1] penalizes narration); the stored enriched body rides
    /// along so a retriever can emit verbatim turn text without re-reading the
    /// conversation blob.
    pub fn search_turns(&self, query: &str, limit: usize) -> Result<Vec<TurnHit>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT t.turn_id, t.session_id, t.salience, t.body,
                   snippet(turns_fts, 1, '>>>', '<<<', '...', 32) as snippet,
                   bm25(turns_fts) * t.salience as score
            FROM turns_fts
            JOIN turns t ON turns_fts.rowid = t.rowid
            WHERE turns_fts MATCH ?1
            ORDER BY score
            LIMIT ?2
            "#,
        )?;

        let rows = stmt.query_map(params![fts_or_query(query), limit as i64], |row| {
            Ok(TurnHit {
                turn_id: row.get(0)?,
                session_id: row.get(1)?,
                salience: row.get(2)?,
                body: row.get(3)?,
                snippet: row.get(4)?,
                score: row.get::<_, f64>(5)?.abs(),
            })
        })?;

        let mut hits = Vec::new();
        for row in rows {
            hits.push(row?);
        }
        Ok(hits)
    }

    /// One indexed turn by id: the dense leg resolves its anchor turns back to
    /// stored text through this instead of re-reading conversation blobs.
    /// `None` for unknown ids and for turns excluded by salience.
    pub fn get_turn(&self, turn_id: &str) -> Result<Option<TurnRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT turn_id, session_id, salience, body FROM turns WHERE turn_id = ?1")?;
        let mut rows = stmt.query_map(params![turn_id], |row| {
            Ok(TurnRow {
                turn_id: row.get(0)?,
                session_id: row.get(1)?,
                salience: row.get(2)?,
                body: row.get(3)?,
            })
        })?;
        rows.next().transpose().map_err(SearchError::Sqlite)
    }

    /// Sessions that *wrote* this path — the authorship signal the
    /// files-touched evidence tier in lineage-retrieval uses; read-only touches
    /// are not evidence (gap 9).
    pub fn sessions_that_wrote_file(&self, file_path: &str) -> Result<Vec<String>> {
        let normalized = normalize_repo_path(file_path, None);
        let mut stmt = self.conn.prepare(
            "SELECT session_id FROM session_files WHERE file_path = ?1 AND wrote = 1 ORDER BY session_id",
        )?;
        let rows = stmt.query_map(params![normalized], |row| row.get::<_, String>(0))?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    pub fn sessions_for_file(&self, file_path: &str) -> Result<Vec<String>> {
        let normalized = normalize_repo_path(file_path, None);
        let mut stmt = self.conn.prepare(
            "SELECT session_id FROM session_files WHERE file_path = ?1 ORDER BY session_id",
        )?;
        let rows = stmt.query_map(params![normalized], |row| row.get::<_, String>(0))?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    /// Replace all stored chunk vectors for a session, tagged with the embedder
    /// `model_version` and each chunk's anchor turn. Delete-then-insert keeps
    /// re-embedding idempotent: the chunk set is derived wholly from the
    /// conversation, so stale chunks must not survive a re-index. Vectors are
    /// stored little-endian; the retriever reads them back the same way.
    pub fn store_session_vectors(
        &self,
        session_id: &str,
        chunks: &[(String, Vec<f32>)],
        model_version: &str,
    ) -> Result<()> {
        // One transaction per session: besides batching fsyncs, it makes an
        // interrupted backfill safe — a session either has all its vectors at
        // the new version or none, never a partial set that would be skipped
        // as already-current.
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "DELETE FROM session_vectors WHERE session_id = ?1",
            params![session_id],
        )?;
        for (chunk_index, (turn_id, vector)) in chunks.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO session_vectors (session_id, chunk_index, turn_id, dim, vector, model_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    session_id,
                    chunk_index as i64,
                    turn_id,
                    vector.len() as i64,
                    vector_to_bytes(vector),
                    model_version,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Session ids already embedded at `model_version` — the embed pass skips
    /// these so a backfill only pays for new or model-changed sessions
    /// (incremental embedding). A session whose vectors are at a different
    /// version is not returned, so it re-embeds.
    pub fn sessions_embedded_at_version(&self, model_version: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT session_id FROM session_vectors WHERE model_version = ?1")?;
        let rows = stmt.query_map(params![model_version], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Every stored chunk vector at `model_version`, for brute-force cosine
    /// scoring. Filtering by version keeps vectors from a previous model or
    /// chunking scheme out of scoring — same-dimension stale vectors would
    /// otherwise mix in silently. At per-repo scale (thousands of chunks)
    /// loading them all is sub-millisecond and needs no ANN index; swap to one
    /// only if the eval shows scale pain.
    pub fn all_chunk_vectors(&self, model_version: &str) -> Result<Vec<ChunkVector>> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id, chunk_index, turn_id, vector FROM session_vectors
             WHERE model_version = ?1",
        )?;
        let rows = stmt.query_map(params![model_version], |row| {
            let bytes: Vec<u8> = row.get(3)?;
            Ok(ChunkVector {
                session_id: row.get(0)?,
                chunk_index: row.get(1)?,
                turn_id: row.get(2)?,
                vector: bytes_to_vector(&bytes),
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Monotonic counter observed by derived caches: any change means the
    /// session corpus may have changed. 0 = nothing indexed yet.
    pub fn generation(&self) -> Result<i64> {
        let value = self
            .conn
            .query_row(
                "SELECT value FROM index_meta WHERE key = 'corpus_generation'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        Ok(value.and_then(|v| v.parse().ok()).unwrap_or(0))
    }

    /// Returns the number of sessions indexed so callers can report it
    /// (diagnostics-v0 `rebuild_index` event) without re-listing refs.
    pub fn rebuild(&self, repo: &Repository) -> Result<usize> {
        self.rebuild_with_progress(repo, &mut |_, _| {})
    }

    /// Like [`rebuild`](Self::rebuild) but calls `progress(done, total)` after
    /// each session is indexed, so the CLI can drive a progress bar without this
    /// crate depending on a rendering library. `total` is the session count
    /// reported once before the loop (as `(0, total)`).
    pub fn rebuild_with_progress(
        &self,
        repo: &Repository,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<usize> {
        self.conn.execute("DELETE FROM sessions", [])?;
        // The delete trigger clears turns_fts alongside.
        self.conn.execute("DELETE FROM turns", [])?;
        self.conn.execute("DELETE FROM session_files", [])?;

        let ids = list_session_ids(repo).map_err(SearchError::Lineage)?;
        let total = ids.len();
        progress(0, total);
        let mut indexed = 0usize;
        for id in ids {
            if let Some(mut conv) =
                read_conversation_stored(repo, &id).map_err(SearchError::Lineage)?
            {
                hydrate_conversation(repo, &mut conv).map_err(SearchError::Lineage)?;
                self.index_conversation(&conv)?;
                indexed += 1;
            }
            progress(indexed, total);
        }
        Ok(indexed)
    }
}

/// Every whitespace-separated word matches as a quoted FTS token, OR-joined,
/// so free text is never parsed as FTS query syntax.
fn fts_or_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|w| format!("\"{w}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// Vectors are stored as raw little-endian f32 bytes — compact and exact, no
/// float-to-text rounding. The dimension is stored alongside so a corrupt or
/// truncated blob is detectable.
fn vector_to_bytes(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for v in vector {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

fn bytes_to_vector(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::{AgentKind, Artifact, ArtifactKind, LineageId, Role, Turn};

    fn open_index() -> (tempfile::TempDir, LineageIndex) {
        let dir = tempfile::tempdir().unwrap();
        let index = LineageIndex::open(dir.path().join("index.db")).unwrap();
        (dir, index)
    }

    fn conversation_touching(workspace_root: &str, paths: &[&str]) -> Conversation {
        let mut conv = Conversation::new(AgentKind::Claude, workspace_root);
        for path in paths {
            conv.turns.push(Turn {
                id: LineageId::new(),
                role: Role::Assistant,
                content: String::new(),
                tool_calls: vec![],
                model: None,
                timestamp: None,
                artifacts: vec![Artifact {
                    kind: ArtifactKind::FileEdit,
                    path: (*path).into(),
                    blob_ref: None,
                    content_hash: None,
                    mime_type: None,
                    preview_data_url: None,
                    line_range: None,
                    resolve: None,
                }],
            });
        }
        conv
    }

    fn chunk(turn_id: &str, vector: Vec<f32>) -> (String, Vec<f32>) {
        (turn_id.to_string(), vector)
    }

    #[test]
    fn session_vectors_round_trip_and_replace_on_reindex() {
        let (_dir, index) = open_index();
        index
            .store_session_vectors(
                "sess-a",
                &[
                    chunk("turn-1", vec![1.0, 0.0, 0.5]),
                    chunk("turn-2", vec![0.0, 1.0, 0.25]),
                ],
                "v1",
            )
            .unwrap();
        index
            .store_session_vectors("sess-b", &[chunk("turn-3", vec![0.1, 0.2, 0.3])], "v1")
            .unwrap();

        let all = index.all_chunk_vectors("v1").unwrap();
        assert_eq!(all.len(), 3);
        let a: Vec<_> = all.iter().filter(|c| c.session_id == "sess-a").collect();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].vector, vec![1.0, 0.0, 0.5]);
        assert_eq!(a[0].turn_id, "turn-1");

        // Re-storing replaces, never accumulates.
        index
            .store_session_vectors("sess-a", &[chunk("turn-9", vec![9.0, 9.0, 9.0])], "v1")
            .unwrap();
        let all = index.all_chunk_vectors("v1").unwrap();
        assert_eq!(all.len(), 2);
        let a: Vec<_> = all.iter().filter(|c| c.session_id == "sess-a").collect();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].vector, vec![9.0, 9.0, 9.0]);
    }

    #[test]
    fn incremental_embedding_tracks_which_sessions_are_current() {
        let (_dir, index) = open_index();
        index
            .store_session_vectors("sess-a", &[chunk("t-a", vec![1.0, 0.0])], "v1")
            .unwrap();
        index
            .store_session_vectors("sess-b", &[chunk("t-b", vec![0.0, 1.0])], "v1")
            .unwrap();
        // sess-c embedded by an older model version.
        index
            .store_session_vectors("sess-c", &[chunk("t-c", vec![0.5, 0.5])], "v0")
            .unwrap();

        let mut current = index.sessions_embedded_at_version("v1").unwrap();
        current.sort();
        assert_eq!(current, vec!["sess-a".to_string(), "sess-b".to_string()]);
        // sess-c is not current, so a v1 pass would re-embed it.
        assert!(!current.contains(&"sess-c".to_string()));
        // Stale-version vectors never reach scoring (all_chunk_vectors filters).
        assert!(index
            .all_chunk_vectors("v1")
            .unwrap()
            .iter()
            .all(|c| c.session_id != "sess-c"));
    }

    #[test]
    fn turn_search_ranks_and_excludes_by_salience() {
        let (_dir, index) = open_index();
        let mut conv = Conversation::new(AgentKind::Claude, "/repo");
        conv.turns.push(Turn {
            id: LineageId::new(),
            role: Role::User,
            content: "we should use redis caching here".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        // A tool-result turn mentioning the same term must never be a hit.
        conv.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Tool,
            content: "caching caching caching build output".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        index.index_conversation(&conv).unwrap();

        let hits = index.search_turns("caching", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].turn_id, conv.turns[0].id.as_str());
        assert_eq!(hits[0].session_id, conv.id.as_str());
        assert!(hits[0].body.contains("redis"));

        // Session-level search aggregates from the same turn corpus.
        let sessions = index.search("caching", 10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, conv.id.as_str());
    }

    #[test]
    fn sessions_for_file_is_an_indexed_lookup_after_import() {
        let (_dir, index) = open_index();
        let conv = conversation_touching("/repo", &["src/auth.rs", "/repo/src/lib.rs"]);
        index.index_conversation(&conv).unwrap();

        assert_eq!(
            index.sessions_for_file("src/auth.rs").unwrap(),
            vec![conv.id.as_str().to_string()]
        );
        // Absolute paths under the workspace root normalize to repo-relative.
        assert_eq!(
            index.sessions_for_file("src/lib.rs").unwrap(),
            vec![conv.id.as_str().to_string()]
        );
        assert!(index.sessions_for_file("src/other.rs").unwrap().is_empty());
    }

    #[test]
    fn reindex_is_idempotent_and_drops_stale_paths() {
        let (_dir, index) = open_index();
        let mut conv = conversation_touching("/repo", &["src/a.rs"]);
        index.index_conversation(&conv).unwrap();
        index.index_conversation(&conv).unwrap();
        assert_eq!(
            index.sessions_for_file("src/a.rs").unwrap(),
            vec![conv.id.as_str().to_string()]
        );

        conv.turns.clear();
        conv.turns
            .extend(conversation_touching("/repo", &["src/b.rs"]).turns);
        index.index_conversation(&conv).unwrap();
        assert!(index.sessions_for_file("src/a.rs").unwrap().is_empty());
        assert_eq!(
            index.sessions_for_file("src/b.rs").unwrap(),
            vec![conv.id.as_str().to_string()]
        );
    }

    #[test]
    fn generation_bumps_on_every_index_write() {
        let (_dir, index) = open_index();
        assert_eq!(index.generation().unwrap(), 0);

        let conv = conversation_touching("/repo", &["src/a.rs"]);
        index.index_conversation(&conv).unwrap();
        let after_first = index.generation().unwrap();
        assert!(after_first > 0);

        index.index_conversation(&conv).unwrap();
        assert!(index.generation().unwrap() > after_first);
    }

    #[test]
    fn wrote_flag_separates_authorship_from_reads() {
        let (_dir, index) = open_index();
        // conversation_touching produces FileEdit artifacts (writes).
        let writer = conversation_touching("/repo", &["src/a.rs"]);
        index.index_conversation(&writer).unwrap();

        // A read-only session: path present in tool_calls but no artifacts.
        let mut reader = Conversation::new(lineage_core::AgentKind::Claude, "/repo");
        reader.turns.push(lineage_core::Turn {
            id: lineage_core::LineageId::new(),
            role: lineage_core::Role::Assistant,
            content: String::new(),
            tool_calls: vec![lineage_core::ToolCall {
                id: "t".into(),
                name: "Read".into(),
                arguments: "{\"file_path\": \"src/a.rs\"}".into(),
                result: None,
            }],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        index.index_conversation(&reader).unwrap();

        // Both touched the file; only the writer is authorship.
        assert_eq!(index.sessions_for_file("src/a.rs").unwrap().len(), 2);
        assert_eq!(
            index.sessions_that_wrote_file("src/a.rs").unwrap(),
            vec![writer.id.as_str().to_string()]
        );
    }
}
