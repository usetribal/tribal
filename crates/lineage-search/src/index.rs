use std::path::Path;

use git2::Repository;
use lineage_core::{Conversation, LineageError};
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
            "#,
        )?;
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

    pub fn rebuild(&self, repo: &Repository) -> Result<()> {
        self.conn.execute("DELETE FROM sessions", [])?;

        let ids = list_session_ids(repo).map_err(SearchError::Lineage)?;
        for id in ids {
            if let Some(mut conv) =
                read_conversation_stored(repo, &id).map_err(SearchError::Lineage)?
            {
                hydrate_conversation(repo, &mut conv).map_err(SearchError::Lineage)?;
                self.index_conversation(&conv)?;
            }
        }
        Ok(())
    }
}
