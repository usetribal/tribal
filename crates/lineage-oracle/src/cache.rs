use std::path::Path;

use rusqlite::{params, Connection};

use crate::retriever::{OracleError, Result};
use crate::types::Retrieval;

/// Version of the serialized `Retrieval` value encoding — not a key part.
/// Key parts answer "would the retriever answer differently?"; this answers
/// "can we still read the answer?" (spec: Cache).
const VALUE_SCHEMA_VERSION: i64 = 1;

/// Everything the retriever's answer depends on. Within one key a hit is
/// exactly what the retriever would say now, which is what makes solo-mode
/// hits provably current — no TTLs (spec: Cache).
#[derive(Debug, Clone)]
pub struct CacheKey<'a> {
    pub file_path: &'a str,
    pub file_blob_sha: &'a str,
    pub corpus_generation: i64,
    pub retriever_version: &'a str,
}

/// Derived-data sidecar: deleting the file is always safe and merely costs
/// re-retrieval. Negative answers (empty retrievals) are stored like any
/// other — answering "nothing" instantly is what keeps the hook invisible.
pub struct OracleCache {
    conn: Connection,
}

impl OracleCache {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent).map_err(|e| OracleError::Cache(e.to_string()))?;
        }
        let conn = Connection::open(path).map_err(|e| OracleError::Cache(e.to_string()))?;
        // WAL + a short busy timeout: parallel hook fires may race; losing a
        // write to contention is fine, blocking the agent's tool call is not.
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| OracleError::Cache(e.to_string()))?;
        conn.pragma_update(None, "busy_timeout", 100)
            .map_err(|e| OracleError::Cache(e.to_string()))?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS retrievals (
                file_path TEXT NOT NULL,
                file_blob_sha TEXT NOT NULL,
                corpus_generation INTEGER NOT NULL,
                retriever_version TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                retrieval TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                PRIMARY KEY (file_path, file_blob_sha, corpus_generation, retriever_version)
            );
            "#,
        )
        .map_err(|e| OracleError::Cache(e.to_string()))?;
        Ok(Self { conn })
    }

    pub fn get(&self, key: &CacheKey) -> Result<Option<Retrieval>> {
        let row = self
            .conn
            .query_row(
                "SELECT schema_version, retrieval FROM retrievals
                 WHERE file_path = ?1 AND file_blob_sha = ?2
                   AND corpus_generation = ?3 AND retriever_version = ?4",
                params![
                    key.file_path,
                    key.file_blob_sha,
                    key.corpus_generation,
                    key.retriever_version
                ],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(OracleError::Cache(other.to_string())),
            })?;

        let Some((schema_version, serialized)) = row else {
            return Ok(None);
        };

        if schema_version != VALUE_SCHEMA_VERSION {
            self.delete(key)?;
            return Ok(None);
        }
        match serde_json::from_str(&serialized) {
            Ok(retrieval) => Ok(Some(retrieval)),
            Err(_) => {
                // Unreadable is a miss, never an error: the entry is derived
                // data and the caller will simply re-retrieve.
                self.delete(key)?;
                Ok(None)
            }
        }
    }

    /// `created_at_unix` is injected by the caller (house rule: no clock
    /// reads inside the system under test); it feeds the team-mode freshness
    /// heuristics and the injection log, not the hit/miss decision.
    pub fn put(&self, key: &CacheKey, retrieval: &Retrieval, created_at_unix: i64) -> Result<()> {
        let serialized =
            serde_json::to_string(retrieval).map_err(|e| OracleError::Cache(e.to_string()))?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO retrievals
                 (file_path, file_blob_sha, corpus_generation, retriever_version,
                  schema_version, retrieval, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    key.file_path,
                    key.file_blob_sha,
                    key.corpus_generation,
                    key.retriever_version,
                    VALUE_SCHEMA_VERSION,
                    serialized,
                    created_at_unix,
                ],
            )
            .map_err(|e| OracleError::Cache(e.to_string()))?;

        // Entries under older generations can never hit again (the key
        // includes the generation), so writing is the moment to shed them.
        self.conn
            .execute(
                "DELETE FROM retrievals WHERE corpus_generation < ?1",
                params![key.corpus_generation],
            )
            .map_err(|e| OracleError::Cache(e.to_string()))?;
        Ok(())
    }

    fn delete(&self, key: &CacheKey) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM retrievals
                 WHERE file_path = ?1 AND file_blob_sha = ?2
                   AND corpus_generation = ?3 AND retriever_version = ?4",
                params![
                    key.file_path,
                    key.file_blob_sha,
                    key.corpus_generation,
                    key.retriever_version
                ],
            )
            .map_err(|e| OracleError::Cache(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Evidence, EvidenceTier, Strength};
    use lineage_core::LineageId;

    fn open_cache() -> (tempfile::TempDir, OracleCache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = OracleCache::open(dir.path().join("oracle.db")).unwrap();
        (dir, cache)
    }

    fn key<'a>(sha: &'a str, generation: i64, version: &'a str) -> CacheKey<'a> {
        CacheKey {
            file_path: "src/auth.rs",
            file_blob_sha: sha,
            corpus_generation: generation,
            retriever_version: version,
        }
    }

    fn retrieval_with_one_hit() -> Retrieval {
        Retrieval::from_evidence(vec![Evidence {
            session_id: LineageId::new(),
            tier: EvidenceTier::FilesTouched,
            strength: Strength::Low,
            match_confidence: None,
            line_ranges: Vec::new(),
            summary: "touched src/auth.rs".into(),
            attribution: "claude session, 2026-07-01".into(),
        }])
    }

    #[test]
    fn hit_after_put_and_miss_on_any_key_part_change() {
        let (_dir, cache) = open_cache();
        let retrieval = retrieval_with_one_hit();
        cache
            .put(&key("aa", 1, "v1"), &retrieval, 1_753_000_000)
            .unwrap();

        assert_eq!(cache.get(&key("aa", 1, "v1")).unwrap(), Some(retrieval));
        // Each key part is a retriever input: file edited, corpus grew, or
        // retriever logic changed must each force a re-retrieve.
        assert_eq!(cache.get(&key("bb", 1, "v1")).unwrap(), None);
        assert_eq!(cache.get(&key("aa", 2, "v1")).unwrap(), None);
        assert_eq!(cache.get(&key("aa", 1, "v2")).unwrap(), None);
    }

    #[test]
    fn negative_answers_are_cached() {
        let (_dir, cache) = open_cache();
        cache
            .put(&key("aa", 1, "v1"), &Retrieval::empty(), 1_753_000_000)
            .unwrap();
        assert_eq!(
            cache.get(&key("aa", 1, "v1")).unwrap(),
            Some(Retrieval::empty())
        );
    }

    #[test]
    fn unreadable_or_mismatched_values_become_misses_and_are_deleted() {
        let (_dir, cache) = open_cache();

        cache
            .conn
            .execute(
                "INSERT INTO retrievals VALUES ('src/auth.rs', 'aa', 1, 'v1', 999, '{}', 0)",
                [],
            )
            .unwrap();
        assert_eq!(cache.get(&key("aa", 1, "v1")).unwrap(), None);

        cache
            .conn
            .execute(
                "INSERT INTO retrievals VALUES ('src/auth.rs', 'bb', 1, 'v1', 1, 'not json', 0)",
                [],
            )
            .unwrap();
        assert_eq!(cache.get(&key("bb", 1, "v1")).unwrap(), None);

        let rows: i64 = cache
            .conn
            .query_row("SELECT COUNT(*) FROM retrievals", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn put_sheds_entries_from_older_generations() {
        let (_dir, cache) = open_cache();
        let retrieval = retrieval_with_one_hit();
        cache.put(&key("aa", 1, "v1"), &retrieval, 0).unwrap();
        cache.put(&key("aa", 2, "v1"), &retrieval, 0).unwrap();

        let rows: i64 = cache
            .conn
            .query_row("SELECT COUNT(*) FROM retrievals", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
        assert_eq!(cache.get(&key("aa", 2, "v1")).unwrap(), Some(retrieval));
    }

    #[test]
    fn deleting_the_cache_file_is_full_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oracle.db");

        let cache = OracleCache::open(&path).unwrap();
        cache
            .put(&key("aa", 1, "v1"), &retrieval_with_one_hit(), 0)
            .unwrap();
        drop(cache);

        std::fs::remove_file(&path).unwrap();
        let cache = OracleCache::open(&path).unwrap();
        assert_eq!(cache.get(&key("aa", 1, "v1")).unwrap(), None);
    }
}
