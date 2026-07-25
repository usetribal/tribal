use std::collections::BTreeMap;
use std::path::Path;

use git2::Repository;
use lineage_core::{
    files_touched, files_written, normalize_repo_path, turn_indexable_text, turn_is_salient,
    turn_salience, Confidence, Conversation, LineObject, LineageError, LineageId,
};
use lineage_git::{
    commit_time, hydrate_conversation, list_line_objects, list_session_ids,
    read_conversation_stored, read_note_for_commit, walk_line_ancestry_shared, AncestryHop,
};
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
    pub salience_class: String,
    pub body: String,
    pub snippet: String,
    pub score: f64,
}

/// One indexed turn, as stored: the salience-admitted enriched text and its
/// session. What `get_turn` returns. `salience_class` is kept for reporting;
/// admission is binary now, so it no longer weights ranking.
#[derive(Debug, Clone)]
pub struct TurnRow {
    pub turn_id: String,
    pub session_id: String,
    pub salience_class: String,
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

/// One line object as mirrored into the `line_objects` table: enough to seed an
/// ancestry walk and to resolve a chain hop to its turn.
#[derive(Debug, Clone)]
pub struct LineObjectRow {
    pub id: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub commit_sha: String,
    pub committed_at: i64,
    pub session_id: String,
    pub turn_id: String,
    pub confidence: String,
}

/// One resolved hop of a temporal chain: the commit that touched the region,
/// its time, and either the attributing turn (when a line object covers the
/// region) or a dark marker (`hop_kind` = dark_no_note | dark_no_match |
/// boundary). Assembled entirely from the two index tables.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Hop {
    pub commit_sha: String,
    pub committed_at: i64,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub confidence: Option<String>,
    pub hop_kind: String,
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

    /// Whether `table` currently has a column named `column`. Used to detect an
    /// older-shaped index on open so it can be dropped and rebuilt.
    fn column_exists(&self, table: &str, column: &str) -> Result<bool> {
        let mut stmt = self.conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            if name == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn init_schema(&self) -> Result<()> {
        // The turn table lost its `salience REAL` column when salience went
        // binary. `CREATE TABLE IF NOT EXISTS` can't alter an existing table, so
        // an index written by an older binary would still have the NOT NULL
        // column and reject the new insert. Drop the old-shaped table (and its
        // FTS mirror) so it is recreated below; the next rebuild repopulates it
        // — the established pre-release upgrade path (rebuild, not migrate).
        let turns_has_salience = self.column_exists("turns", "salience")?;
        if turns_has_salience {
            self.conn.execute_batch(
                r#"
                DROP TRIGGER IF EXISTS turns_ai;
                DROP TRIGGER IF EXISTS turns_ad;
                DROP TRIGGER IF EXISTS turns_au;
                DROP TABLE IF EXISTS turns_fts;
                DROP TABLE IF EXISTS turns;
                "#,
            )?;
        }
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
            CREATE TABLE IF NOT EXISTS session_commits (
                session_id TEXT NOT NULL,
                commit_sha TEXT NOT NULL,
                PRIMARY KEY (session_id, commit_sha)
            );
            CREATE INDEX IF NOT EXISTS idx_session_commits_commit
                ON session_commits(commit_sha);
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
            CREATE TABLE IF NOT EXISTS line_objects (
                id TEXT PRIMARY KEY,
                file_path TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                commit_sha TEXT NOT NULL,
                committed_at INTEGER NOT NULL,
                session_id TEXT NOT NULL,
                turn_id TEXT NOT NULL,
                confidence TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_line_objects_file_commit
                ON line_objects(file_path, commit_sha);
            CREATE INDEX IF NOT EXISTS idx_line_objects_file_time
                ON line_objects(file_path, committed_at);
            CREATE INDEX IF NOT EXISTS idx_line_objects_turn
                ON line_objects(turn_id);
            CREATE TABLE IF NOT EXISTS line_ancestry (
                file_path TEXT NOT NULL,
                commit_sha TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                parent_commit_sha TEXT,
                parent_file_path TEXT,
                parent_start_line INTEGER,
                parent_end_line INTEGER,
                hop_kind TEXT NOT NULL,
                PRIMARY KEY (file_path, commit_sha, start_line, end_line)
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

        // The FTS document is the turn. Salience admission is binary: tool
        // results and pure exploration are excluded entirely (this is what
        // stops a session's exploratory open from ranking it on incidental
        // matches), while everything else — including narration — is indexed at
        // parity and ranked by plain bm25. The class is stored for reporting.
        // Delete-then-insert keeps re-imports idempotent, same as session_files.
        self.conn.execute(
            "DELETE FROM turns WHERE session_id = ?1",
            params![conversation.id.as_str()],
        )?;
        for (turn_index, turn) in conversation.turns.iter().enumerate() {
            if !turn_is_salient(turn) {
                continue;
            }
            let body = turn_indexable_text(turn);
            if body.is_empty() {
                continue;
            }
            self.conn.execute(
                "INSERT OR REPLACE INTO turns (turn_id, session_id, turn_index, salience_class, body)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    turn.id.as_str(),
                    conversation.id.as_str(),
                    turn_index as i64,
                    turn_salience(turn).as_str(),
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

        // The session↔commit edge lives in the conversation ref (git notes are
        // the other half); mirroring it here is what makes `sessions-for-commit`
        // one indexed lookup instead of a scan of every session ref. Same
        // delete-then-insert discipline as session_files.
        self.conn.execute(
            "DELETE FROM session_commits WHERE session_id = ?1",
            params![conversation.id.as_str()],
        )?;
        for commit_sha in &conversation.commit_shas {
            self.conn.execute(
                "INSERT OR IGNORE INTO session_commits (session_id, commit_sha) VALUES (?1, ?2)",
                params![conversation.id.as_str(), commit_sha],
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
    /// its best (plain-bm25) turn, shown with that turn's snippet. One
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

    /// Turn-level search: the intent-retrieval unit. Ranked by plain bm25
    /// (negative, lower = better) over the binary-salient corpus — narration is
    /// indexed at parity, so ranking is relevance alone and the FTS leg agrees
    /// with the dense leg on which turns matter. The stored enriched body rides
    /// along so a retriever can emit verbatim turn text without re-reading the
    /// conversation blob.
    pub fn search_turns(&self, query: &str, limit: usize) -> Result<Vec<TurnHit>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT t.turn_id, t.session_id, t.salience_class, t.body,
                   snippet(turns_fts, 1, '>>>', '<<<', '...', 32) as snippet,
                   bm25(turns_fts) as score
            FROM turns_fts
            JOIN turns t ON turns_fts.rowid = t.rowid
            WHERE turns_fts MATCH ?1
            ORDER BY score
            LIMIT ?2
            "#,
        )?;

        let rows = stmt.query_map(params![fts_or_query(query), limit as i64], row_to_turn_hit)?;

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
        let mut stmt = self.conn.prepare(
            "SELECT turn_id, session_id, salience_class, body FROM turns WHERE turn_id = ?1",
        )?;
        let mut rows = stmt.query_map(params![turn_id], row_to_turn_row)?;
        rows.next().transpose().map_err(SearchError::Sqlite)
    }

    /// Turn-level search restricted to a session set — the batching win behind
    /// the `search-within` verb: one indexed query instead of N greps over
    /// materialized transcripts. An empty session set matches nothing rather
    /// than everything, so a caller that lost its session list gets silence, not
    /// the whole corpus.
    pub fn search_turns_in_sessions(
        &self,
        session_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<TurnHit>> {
        if session_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = sql_placeholders(session_ids.len(), 3);
        let mut stmt = self.conn.prepare(&format!(
            r#"
            SELECT t.turn_id, t.session_id, t.salience_class, t.body,
                   snippet(turns_fts, 1, '>>>', '<<<', '...', 32) as snippet,
                   bm25(turns_fts) as score
            FROM turns_fts
            JOIN turns t ON turns_fts.rowid = t.rowid
            WHERE turns_fts MATCH ?1 AND t.session_id IN ({placeholders})
            ORDER BY score
            LIMIT ?2
            "#
        ))?;

        let mut args: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(fts_or_query(query)), Box::new(limit as i64)];
        args.extend(
            session_ids
                .iter()
                .map(|id| Box::new(id.clone()) as Box<dyn rusqlite::ToSql>),
        );
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), row_to_turn_hit)?;

        let mut hits = Vec::new();
        for row in rows {
            hits.push(row?);
        }
        Ok(hits)
    }

    /// The turns immediately before and after `turn_id` in its own session,
    /// within `radius` positions, in conversation order — what the `around` verb
    /// reads. `turns.turn_index` finally earns its column here. A turn unknown to
    /// the index (or dropped by salience) yields nothing rather than an error:
    /// an agent following a stale handle should see silence.
    pub fn turns_around(&self, turn_id: &str, radius: u32, limit: usize) -> Result<Vec<TurnRow>> {
        let Some(anchor) = self.turn_position(turn_id)? else {
            return Ok(Vec::new());
        };
        let (session_id, turn_index) = anchor;
        let mut stmt = self.conn.prepare(
            "SELECT turn_id, session_id, salience_class, body FROM turns
             WHERE session_id = ?1 AND turn_index BETWEEN ?2 AND ?3
             ORDER BY turn_index
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![
                session_id,
                turn_index - i64::from(radius),
                turn_index + i64::from(radius),
                limit as i64,
            ],
            row_to_turn_row,
        )?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Where an indexed turn sits: its session and position in it.
    fn turn_position(&self, turn_id: &str) -> Result<Option<(String, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT session_id, turn_index FROM turns WHERE turn_id = ?1")?;
        let mut rows = stmt.query_map(params![turn_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.next().transpose().map_err(SearchError::Sqlite)
    }

    /// The line objects a turn produced, most recent first — `idx_line_objects_turn`
    /// read in the direction nothing used before, which is what makes the graph
    /// two-way (turn → code, not only code → turn).
    pub fn line_objects_for_turn(&self, turn_id: &str, limit: usize) -> Result<Vec<LineObjectRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, file_path, start_line, end_line, commit_sha, committed_at,
                    session_id, turn_id, confidence
             FROM line_objects WHERE turn_id = ?1
             ORDER BY committed_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![turn_id, limit as i64], row_to_line_object)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Sessions linked to a commit, from the `session_commits` mirror — the one
    /// traversal whose entry point is ordinary git work rather than an injected
    /// digest.
    pub fn sessions_for_commit(&self, commit_sha: &str, limit: usize) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id FROM session_commits WHERE commit_sha = ?1
             ORDER BY session_id LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![commit_sha, limit as i64], |row| {
            row.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Record one session↔commit edge without re-indexing the session. The link
    /// paths (post-commit hook, `git lineage link`) write the edge to the
    /// conversation ref but never re-run `index_conversation`, so the mirror
    /// would otherwise go stale until the next rebuild.
    pub fn link_session_commit(&self, session_id: &str, commit_sha: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO session_commits (session_id, commit_sha) VALUES (?1, ?2)",
            params![session_id, commit_sha],
        )?;
        Ok(())
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

    /// Depth cap on an ancestry walk: a squash/rebase floor or a long-lived
    /// line bottoms out honestly at a boundary before this, but a pathological
    /// history must not walk unbounded. The plan's "boundary/depth cap".
    const ANCESTRY_MAX_HOPS: usize = 50;

    /// Insert one line object's mirror row. The commit's time is looked up once
    /// per object; an object whose commit no longer exists is skipped (returns
    /// `false`) — it points at unreachable history and can never anchor a chain.
    fn insert_line_object_row(&self, repo: &Repository, obj: &LineObject) -> Result<bool> {
        let Some(committed_at) = commit_time(repo, &obj.commit_sha)? else {
            return Ok(false);
        };
        let normalized = normalize_repo_path(&obj.file_path, None);
        self.conn.execute(
            "INSERT OR REPLACE INTO line_objects
             (id, file_path, start_line, end_line, commit_sha, committed_at,
              session_id, turn_id, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                obj.id.as_str(),
                normalized,
                obj.line_range[0] as i64,
                obj.line_range[1] as i64,
                obj.commit_sha,
                committed_at,
                obj.conversation_id.as_str(),
                obj.turn_id.as_str(),
                confidence_str(obj),
            ],
        )?;
        Ok(true)
    }

    /// Walk the blame ancestry of one line object's region and upsert an edge
    /// per hop. The whole region is blamed once per hop (hunk-grain), so a
    /// region that splits across commits diverges into distinct edges and
    /// containment at query time picks the right one; a single-commit region
    /// stays one edge. `hop_kind` is decided per child commit from
    /// note/line-object presence, so a dark hop still records a continuable edge.
    fn populate_ancestry_for_region(
        &self,
        repo: &Repository,
        file_path: &str,
        commit_sha: &str,
        start_line: u32,
        end_line: u32,
        seen: &mut std::collections::HashSet<(String, u32, u32)>,
    ) -> Result<()> {
        let hops = walk_line_ancestry_shared(
            repo,
            commit_sha,
            file_path,
            start_line,
            end_line,
            Self::ANCESTRY_MAX_HOPS,
            seen,
        )?;
        for hop in &hops {
            let hop_kind = self.hop_kind_for(repo, hop)?;
            // PK (file, commit, start, end) makes exact-duplicate edges from
            // overlapping seeds idempotent; INSERT OR IGNORE keeps the first
            // writer's edge (they agree on parent — same blame).
            self.conn.execute(
                "INSERT OR IGNORE INTO line_ancestry
                 (file_path, commit_sha, start_line, end_line,
                  parent_commit_sha, parent_file_path, parent_start_line, parent_end_line,
                  hop_kind)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    hop.file_path,
                    hop.commit_sha,
                    hop.start_line as i64,
                    hop.end_line as i64,
                    hop.parent.as_ref().map(|p| p.commit_sha.clone()),
                    hop.parent.as_ref().map(|p| p.file_path.clone()),
                    hop.parent.as_ref().map(|p| p.start_line as i64),
                    hop.parent.as_ref().map(|p| p.end_line as i64),
                    hop_kind,
                ],
            )?;
        }
        Ok(())
    }

    /// Classify a hop: a boundary (no parent) records honestly; otherwise the
    /// edge is `resolved` if a line object covers the child region, `dark_no_note`
    /// if the child commit has no lineage note, else `dark_no_match` (note
    /// present but no covering object — carried-along region).
    fn hop_kind_for(&self, repo: &Repository, hop: &AncestryHop) -> Result<String> {
        if hop.parent.is_none() {
            return Ok("boundary".to_string());
        }
        if self
            .line_object_covering(&hop.file_path, &hop.commit_sha, hop.start_line)?
            .is_some()
        {
            return Ok("resolved".to_string());
        }
        let has_note = read_note_for_commit(repo, &hop.commit_sha)
            .map_err(SearchError::Lineage)?
            .is_some();
        Ok(if has_note {
            "dark_no_match".to_string()
        } else {
            "dark_no_note".to_string()
        })
    }

    /// The narrowest line object whose region contains `line` at `(file,
    /// commit)`, or `None`. Narrowest wins: the tightest range is the most
    /// specific attribution (the prototype's `resolve_turn` rule).
    fn line_object_covering(
        &self,
        file_path: &str,
        commit_sha: &str,
        line: u32,
    ) -> Result<Option<LineObjectRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, file_path, start_line, end_line, commit_sha, committed_at,
                    session_id, turn_id, confidence
             FROM line_objects
             WHERE file_path = ?1 AND commit_sha = ?2
               AND start_line <= ?3 AND end_line >= ?3
             ORDER BY (end_line - start_line) ASC
             LIMIT 1",
        )?;
        let mut rows = stmt.query_map(
            params![file_path, commit_sha, line as i64],
            row_to_line_object,
        )?;
        rows.next().transpose().map_err(SearchError::Sqlite)
    }

    /// Full recompute of both line tables: wipe, mirror every reachable line
    /// object, then seed an ancestry walk from each object's region. Batched in
    /// one transaction — the repo learned autocommit fsyncs make a rebuild
    /// dominate wall time. `progress(done, total)` fires per line object walked.
    pub fn populate_line_tables(
        &self,
        repo: &Repository,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute("DELETE FROM line_objects", [])?;
        self.conn.execute("DELETE FROM line_ancestry", [])?;

        let objects = list_line_objects(repo).map_err(SearchError::Lineage)?;
        let total = objects.len();
        progress(0, total);

        // Two passes: mirror every object first so the ancestry pass can decide
        // `resolved` vs dark from a complete line_objects table, not a partial
        // one — a hop's child commit may host objects not yet inserted.
        let mut mirrored = Vec::with_capacity(objects.len());
        for obj in &objects {
            if self.insert_line_object_row(repo, obj)? {
                mirrored.push(obj);
            }
        }
        // One seen-set shared across every seed: thousands of overlapping
        // regions share ancestry commits, so a blame position is walked once
        // for the whole pass, not once per line object.
        let mut seen = std::collections::HashSet::new();
        for (done, obj) in mirrored.iter().enumerate() {
            let file_path = normalize_repo_path(&obj.file_path, None);
            self.populate_ancestry_for_region(
                repo,
                &file_path,
                &obj.commit_sha,
                obj.line_range[0],
                obj.line_range[1],
                &mut seen,
            )?;
            progress(done + 1, total);
        }

        tx.commit()?;
        Ok(mirrored.len())
    }

    /// Incremental line-table population for one session set's line objects
    /// (import / post-commit link path). Refreshes those objects' mirror rows
    /// and seeds ancestry from their regions; existing edges are left in place
    /// (they are derived from unchanged history). Batched in one transaction.
    pub fn populate_line_tables_for_sessions(
        &self,
        repo: &Repository,
        session_ids: &[LineageId],
    ) -> Result<usize> {
        if session_ids.is_empty() {
            return Ok(0);
        }
        let wanted: std::collections::HashSet<&str> =
            session_ids.iter().map(|id| id.as_str()).collect();
        let objects: Vec<LineObject> = list_line_objects(repo)
            .map_err(SearchError::Lineage)?
            .into_iter()
            .filter(|o| wanted.contains(o.conversation_id.as_str()))
            .collect();

        let tx = self.conn.unchecked_transaction()?;
        let mut mirrored = 0usize;
        for obj in &objects {
            if !self.insert_line_object_row(repo, obj)? {
                continue;
            }
            mirrored += 1;
        }
        let mut seen = std::collections::HashSet::new();
        for obj in &objects {
            let file_path = normalize_repo_path(&obj.file_path, None);
            self.populate_ancestry_for_region(
                repo,
                &file_path,
                &obj.commit_sha,
                obj.line_range[0],
                obj.line_range[1],
                &mut seen,
            )?;
        }
        tx.commit()?;
        Ok(mirrored)
    }

    /// Resolve a temporal chain for `(file_path, line)` anchored at
    /// `anchor_commit` (the commit a single live HEAD blame resolved the line
    /// to). Everything after the anchor is indexed reads: no `Repository` is
    /// taken, so this function cannot blame. Each hop is the ancestry edge at
    /// the current position plus (for resolved hops) the covering line object.
    pub fn line_history(
        &self,
        file_path: &str,
        line: u32,
        anchor_commit: &str,
    ) -> Result<Vec<Hop>> {
        let file_path = normalize_repo_path(file_path, None);
        let mut hops = Vec::new();
        let mut cur_file = file_path;
        let mut cur_commit = anchor_commit.to_string();
        let mut cur_line = line;
        let mut seen = std::collections::HashSet::new();

        for _ in 0..Self::ANCESTRY_MAX_HOPS {
            let Some(edge) = self.ancestry_edge_covering(&cur_file, &cur_commit, cur_line)? else {
                break;
            };
            // A revisited (commit, line) means a cycle in stored edges — bail
            // rather than loop; the index is derived and could be inconsistent.
            if !seen.insert((edge.commit_sha.clone(), edge.start_line)) {
                break;
            }
            hops.push(self.hop_from_edge(&edge)?);

            let Some(parent) = edge.parent else {
                break;
            };
            cur_file = parent.file_path;
            cur_commit = parent.commit_sha;
            cur_line = parent.start_line;
        }
        Ok(hops)
    }

    /// The ancestry edge whose child region contains `line` at `(file, commit)`,
    /// narrowest first (a sub-range divergence stores two edges; containment
    /// picks the tighter, more specific one).
    fn ancestry_edge_covering(
        &self,
        file_path: &str,
        commit_sha: &str,
        line: u32,
    ) -> Result<Option<AncestryEdge>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path, commit_sha, start_line, end_line,
                    parent_commit_sha, parent_file_path, parent_start_line, parent_end_line,
                    hop_kind
             FROM line_ancestry
             WHERE file_path = ?1 AND commit_sha = ?2
               AND start_line <= ?3 AND end_line >= ?3
             ORDER BY (end_line - start_line) ASC
             LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![file_path, commit_sha, line as i64], |row| {
            let parent_commit: Option<String> = row.get(4)?;
            let parent = parent_commit.map(|commit_sha| AncestryEdgeParent {
                commit_sha,
                file_path: row
                    .get::<_, Option<String>>(5)
                    .ok()
                    .flatten()
                    .unwrap_or_default(),
                start_line: row.get::<_, Option<i64>>(6).ok().flatten().unwrap_or(0) as u32,
            });
            Ok(AncestryEdge {
                file_path: row.get(0)?,
                commit_sha: row.get(1)?,
                start_line: row.get::<_, i64>(2)? as u32,
                end_line: row.get::<_, i64>(3)? as u32,
                hop_kind: row.get(8)?,
                parent,
            })
        })?;
        rows.next().transpose().map_err(SearchError::Sqlite)
    }

    /// Decorate an edge into a `Hop`: a resolved edge carries the covering line
    /// object's turn; a dark/boundary edge carries only its kind. `committed_at`
    /// comes from a covering line object when one exists, else 0 (a dark hop's
    /// child commit has no object to source it from).
    fn hop_from_edge(&self, edge: &AncestryEdge) -> Result<Hop> {
        let covering =
            self.line_object_covering(&edge.file_path, &edge.commit_sha, edge.start_line)?;
        let (session_id, turn_id, confidence, committed_at) = match &covering {
            Some(obj) => (
                Some(obj.session_id.clone()),
                Some(obj.turn_id.clone()),
                Some(obj.confidence.clone()),
                obj.committed_at,
            ),
            None => (None, None, None, 0),
        };
        Ok(Hop {
            commit_sha: edge.commit_sha.clone(),
            committed_at,
            file_path: edge.file_path.clone(),
            start_line: edge.start_line,
            end_line: edge.end_line,
            session_id,
            turn_id,
            confidence,
            hop_kind: edge.hop_kind.clone(),
        })
    }

    /// All turns that ever touched `file_path`, most recent first — the
    /// aggregation query `committed_at` earns its column for ("why so bloated").
    /// No walking: a single indexed scan of the mirror table.
    pub fn line_objects_for_file(&self, file_path: &str) -> Result<Vec<LineObjectRow>> {
        let normalized = normalize_repo_path(file_path, None);
        let mut stmt = self.conn.prepare(
            "SELECT id, file_path, start_line, end_line, commit_sha, committed_at,
                    session_id, turn_id, confidence
             FROM line_objects WHERE file_path = ?1 ORDER BY committed_at DESC",
        )?;
        let rows = stmt.query_map(params![normalized], row_to_line_object)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Every recorded line span, grouped by file — the input to doctor's
    /// coverage section. Reading the mirror table costs one scan; deriving the
    /// same answer from `refs/lineage/lines/*` costs a `cat-file` per object.
    /// Degenerate spans (`end_line` before `start_line`) are dropped here so
    /// callers never have to defend against them.
    pub fn coverage_spans(&self) -> Result<BTreeMap<String, Vec<(u32, u32)>>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path, start_line, end_line FROM line_objects
             WHERE end_line >= start_line",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, u32>(2)?,
            ))
        })?;
        let mut by_file: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
        for row in rows {
            let (file_path, start_line, end_line) = row?;
            by_file
                .entry(file_path)
                .or_default()
                .push((start_line, end_line));
        }
        Ok(by_file)
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
        self.conn.execute("DELETE FROM session_commits", [])?;

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

/// One stored ancestry edge with its parent side, as read back for the walk.
struct AncestryEdge {
    file_path: String,
    commit_sha: String,
    start_line: u32,
    end_line: u32,
    hop_kind: String,
    parent: Option<AncestryEdgeParent>,
}

struct AncestryEdgeParent {
    commit_sha: String,
    file_path: String,
    start_line: u32,
}

fn row_to_turn_hit(row: &rusqlite::Row) -> rusqlite::Result<TurnHit> {
    Ok(TurnHit {
        turn_id: row.get(0)?,
        session_id: row.get(1)?,
        salience_class: row.get(2)?,
        body: row.get(3)?,
        snippet: row.get(4)?,
        score: row.get::<_, f64>(5)?.abs(),
    })
}

fn row_to_turn_row(row: &rusqlite::Row) -> rusqlite::Result<TurnRow> {
    Ok(TurnRow {
        turn_id: row.get(0)?,
        session_id: row.get(1)?,
        salience_class: row.get(2)?,
        body: row.get(3)?,
    })
}

/// `?n, ?n+1, …` for an IN clause of `count` values starting at `?first`.
/// SQLite has no array binding, so a variable-length IN list must be built as
/// text; the values themselves still bind as parameters.
fn sql_placeholders(count: usize, first: usize) -> String {
    (first..first + count)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn row_to_line_object(row: &rusqlite::Row) -> rusqlite::Result<LineObjectRow> {
    Ok(LineObjectRow {
        id: row.get(0)?,
        file_path: row.get(1)?,
        start_line: row.get::<_, i64>(2)? as u32,
        end_line: row.get::<_, i64>(3)? as u32,
        commit_sha: row.get(4)?,
        committed_at: row.get(5)?,
        session_id: row.get(6)?,
        turn_id: row.get(7)?,
        confidence: row.get(8)?,
    })
}

/// The line object's confidence as its stored string (exact | heuristic |
/// manual), matching the enum's lowercase serde spelling.
fn confidence_str(obj: &LineObject) -> &'static str {
    match obj.confidence {
        Confidence::Exact => "exact",
        Confidence::Heuristic => "heuristic",
        Confidence::Manual => "manual",
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

    /// A session whose turns are salient prose, so the traversal verbs have a
    /// corpus to walk. Bodies differ so FTS can pick one out.
    fn conversation_saying(workspace_root: &str, prompts: &[&str]) -> Conversation {
        let mut conv = Conversation::new(AgentKind::Claude, workspace_root);
        for prompt in prompts {
            conv.turns.push(Turn {
                id: LineageId::new(),
                role: Role::User,
                content: (*prompt).into(),
                tool_calls: vec![],
                model: None,
                timestamp: None,
                artifacts: vec![],
            });
        }
        conv
    }

    #[test]
    fn scoped_search_stays_inside_the_given_sessions() {
        let (_dir, index) = open_index();
        let inside = conversation_saying("/repo", &["we chose redis for the session cache"]);
        let outside = conversation_saying("/repo", &["redis appears here too but out of scope"]);
        index.index_conversation(&inside).unwrap();
        index.index_conversation(&outside).unwrap();

        let hits = index
            .search_turns_in_sessions(&[inside.id.as_str().to_string()], "redis", 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, inside.id.as_str());

        // Global search still sees both, so the scoping is the filter and not a
        // corpus difference.
        assert_eq!(index.search_turns("redis", 10).unwrap().len(), 2);
    }

    #[test]
    fn scoped_search_with_no_sessions_matches_nothing() {
        let (_dir, index) = open_index();
        let conv = conversation_saying("/repo", &["redis"]);
        index.index_conversation(&conv).unwrap();
        assert!(index
            .search_turns_in_sessions(&[], "redis", 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn turns_around_returns_the_neighbourhood_in_conversation_order() {
        let (_dir, index) = open_index();
        let conv = conversation_saying("/repo", &["first", "second", "third", "fourth", "fifth"]);
        index.index_conversation(&conv).unwrap();

        let around = index
            .turns_around(conv.turns[2].id.as_str(), 1, 10)
            .unwrap();
        let bodies: Vec<&str> = around.iter().map(|t| t.body.as_str()).collect();
        assert_eq!(bodies, vec!["second", "third", "fourth"]);

        // The bound is honoured even when the radius would reach further.
        assert_eq!(
            index
                .turns_around(conv.turns[2].id.as_str(), 2, 3)
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn turns_around_clamps_at_session_edges_and_is_silent_on_unknown_turns() {
        let (_dir, index) = open_index();
        let conv = conversation_saying("/repo", &["first", "second"]);
        index.index_conversation(&conv).unwrap();

        let around = index
            .turns_around(conv.turns[0].id.as_str(), 5, 10)
            .unwrap();
        assert_eq!(around.len(), 2, "never walks past the session boundary");
        assert!(index
            .turns_around("no-such-turn", 1, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn session_commits_mirror_resolves_a_commit_to_its_sessions() {
        let (_dir, index) = open_index();
        let mut conv = conversation_saying("/repo", &["the deciding turn"]);
        conv.commit_shas.push("a".repeat(40));
        index.index_conversation(&conv).unwrap();

        assert_eq!(
            index.sessions_for_commit(&"a".repeat(40), 10).unwrap(),
            vec![conv.id.as_str().to_string()]
        );
        assert!(index
            .sessions_for_commit(&"b".repeat(40), 10)
            .unwrap()
            .is_empty());

        // A link made after indexing (post-commit hook, `git lineage link`)
        // reaches the mirror without a re-index.
        index
            .link_session_commit(conv.id.as_str(), &"b".repeat(40))
            .unwrap();
        assert_eq!(
            index.sessions_for_commit(&"b".repeat(40), 10).unwrap(),
            vec![conv.id.as_str().to_string()]
        );
    }

    #[test]
    fn session_commits_are_replaced_not_accumulated_on_reindex() {
        let (_dir, index) = open_index();
        let mut conv = conversation_saying("/repo", &["turn"]);
        conv.commit_shas.push("a".repeat(40));
        index.index_conversation(&conv).unwrap();

        conv.commit_shas = vec!["c".repeat(40)];
        index.index_conversation(&conv).unwrap();
        assert!(index
            .sessions_for_commit(&"a".repeat(40), 10)
            .unwrap()
            .is_empty());
        assert_eq!(
            index
                .sessions_for_commit(&"c".repeat(40), 10)
                .unwrap()
                .len(),
            1
        );
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

    /// Mirror rows normally arrive via `insert_line_object_row`, which needs a
    /// real commit; coverage only reads three columns, so the rows are written
    /// directly to keep the test about the query.
    fn insert_span(index: &LineageIndex, id: &str, file_path: &str, start: i64, end: i64) {
        index
            .conn
            .execute(
                "INSERT INTO line_objects
                 (id, file_path, start_line, end_line, commit_sha, committed_at,
                  session_id, turn_id, confidence)
                 VALUES (?1, ?2, ?3, ?4, 'sha', 0, 'sess', 'turn', 'exact')",
                params![id, file_path, start, end],
            )
            .unwrap();
    }

    #[test]
    fn coverage_spans_groups_by_file_and_drops_degenerate_rows() {
        let (_dir, index) = open_index();
        insert_span(&index, "a", "src/a.rs", 1, 10);
        insert_span(&index, "b", "src/a.rs", 20, 25);
        insert_span(&index, "c", "src/b.rs", 3, 3);
        insert_span(&index, "d", "src/b.rs", 9, 4);

        let spans = index.coverage_spans().unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans["src/a.rs"], vec![(1, 10), (20, 25)]);
        assert_eq!(spans["src/b.rs"], vec![(3, 3)]);
    }

    #[test]
    fn coverage_spans_is_empty_for_a_fresh_index() {
        let (_dir, index) = open_index();
        assert!(index.coverage_spans().unwrap().is_empty());
    }
}
