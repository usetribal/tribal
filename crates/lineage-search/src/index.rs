use std::path::Path;

use git2::Repository;
use lineage_core::{files_touched, files_written, normalize_repo_path, Conversation, LineageError};
use lineage_git::{
    hydrate_conversation, indexable_body, list_session_ids, read_conversation_stored,
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
                body TEXT NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS sessions_fts USING fts5(
                id UNINDEXED,
                body,
                content='sessions',
                content_rowid='rowid'
            );
            CREATE TRIGGER IF NOT EXISTS sessions_ai AFTER INSERT ON sessions BEGIN
                INSERT INTO sessions_fts(rowid, body) VALUES (new.rowid, new.body);
            END;
            CREATE TRIGGER IF NOT EXISTS sessions_ad AFTER DELETE ON sessions BEGIN
                INSERT INTO sessions_fts(sessions_fts, rowid, body) VALUES('delete', old.rowid, old.body);
            END;
            CREATE TRIGGER IF NOT EXISTS sessions_au AFTER UPDATE ON sessions BEGIN
                INSERT INTO sessions_fts(sessions_fts, rowid, body) VALUES('delete', old.rowid, old.body);
                INSERT INTO sessions_fts(rowid, body) VALUES (new.rowid, new.body);
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
            "#,
        )?;
        // Indexes created by binaries predating the read/write distinction
        // lack the column; rows stay wrote=0 until the next (re)index.
        let _ = self.conn.execute(
            "ALTER TABLE session_files ADD COLUMN wrote INTEGER NOT NULL DEFAULT 0",
            [],
        );
        Ok(())
    }

    pub fn index_conversation(&self, conversation: &Conversation) -> Result<()> {
        let body = indexable_body(conversation);

        self.conn.execute(
            "INSERT OR REPLACE INTO sessions (id, agent, started_at, body) VALUES (?1, ?2, ?3, ?4)",
            params![
                conversation.id.as_str(),
                conversation.agent.as_str(),
                conversation.started_at.to_rfc3339(),
                body,
            ],
        )?;

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

        // The generation is what lets a derived cache (lineage-oracle) detect
        // that the session corpus changed without being told about imports.
        self.conn.execute(
            "INSERT INTO index_meta (key, value) VALUES ('corpus_generation', '1')
             ON CONFLICT(key) DO UPDATE SET value = CAST(value AS INTEGER) + 1",
            [],
        )?;

        Ok(())
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT s.id, snippet(sessions_fts, 1, '>>>', '<<<', '...', 32) as snippet,
                   bm25(sessions_fts) as score
            FROM sessions_fts
            JOIN sessions s ON sessions_fts.rowid = s.rowid
            WHERE sessions_fts MATCH ?1
            ORDER BY score
            LIMIT ?2
            "#,
        )?;

        let fts_query = query
            .split_whitespace()
            .map(|w| format!("\"{w}\""))
            .collect::<Vec<_>>()
            .join(" OR ");

        let rows = stmt.query_map(params![fts_query, limit as i64], |row| {
            Ok(SearchHit {
                session_id: row.get(0)?,
                snippet: row.get(1)?,
                score: row.get::<_, f64>(2)?.abs(),
            })
        })?;

        let mut hits = Vec::new();
        for row in rows {
            hits.push(row?);
        }
        Ok(hits)
    }

    /// Sessions whose tool calls touched this repo-relative path, most useful
    /// as the files-touched evidence tier in lineage-oracle.
    /// Sessions that *wrote* this path — the authorship signal the oracle's
    /// evidence tier uses; read-only touches are not evidence (gap 9).
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
        self.conn.execute("DELETE FROM sessions", [])?;
        self.conn.execute("DELETE FROM session_files", [])?;

        let ids = list_session_ids(repo).map_err(SearchError::Lineage)?;
        let mut indexed = 0usize;
        for id in ids {
            if let Some(mut conv) =
                read_conversation_stored(repo, &id).map_err(SearchError::Lineage)?
            {
                hydrate_conversation(repo, &mut conv).map_err(SearchError::Lineage)?;
                self.index_conversation(&conv)?;
                indexed += 1;
            }
        }
        Ok(indexed)
    }
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
