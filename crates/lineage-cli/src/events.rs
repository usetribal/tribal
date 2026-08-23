//! Append-only local event log at `.git/lineage/events.jsonl` — the one place
//! to look for what `tribal` did (spec: `specs/diagnostics-v0.md`).
//!
//! Best-effort by contract: a failed write becomes a `tracing` warning and
//! never an `Err` to the operation being recorded — the same fail-open posture
//! the context hook has, generalized. Timestamps are always passed in by the
//! caller so this module never reads the clock.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};

pub const EVENTS_SCHEMA_VERSION: &str = "lineage-events-v0";
const EVENTS_FILE: &str = "events.jsonl";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Error,
    Silent,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Error => "error",
            Outcome::Silent => "silent",
        }
    }
}

pub struct EventLog {
    dir: PathBuf,
}

impl EventLog {
    pub fn for_git_dir(git_dir: &Path) -> Self {
        Self {
            dir: git_dir.join("lineage"),
        }
    }

    /// Best-effort handle from a workdir path for callers that never open the
    /// repo themselves; `None` (nothing logged) when it cannot be opened.
    pub fn for_repo_path(repo_path: &Path) -> Option<Self> {
        let repo = lineage_git::open_repo(repo_path).ok()?;
        Some(Self::for_git_dir(&repo.git_dir()))
    }

    pub fn append(&self, ts: DateTime<Utc>, op: &str, outcome: Outcome, detail: serde_json::Value) {
        if let Err(e) = self.try_append(ts, op, outcome, detail) {
            tracing::warn!("event log write failed: {e}");
        }
    }

    fn try_append(
        &self,
        ts: DateTime<Utc>,
        op: &str,
        outcome: Outcome,
        detail: serde_json::Value,
    ) -> std::io::Result<()> {
        let entry = serde_json::json!({
            "schema_version": EVENTS_SCHEMA_VERSION,
            "ts": ts.to_rfc3339_opts(SecondsFormat::Secs, true),
            "op": op,
            "outcome": outcome.as_str(),
            "detail": detail,
        });
        fs::create_dir_all(&self.dir)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(EVENTS_FILE))?;
        writeln!(file, "{entry}")
    }

    /// All parseable entries, oldest first. Unparsable lines are skipped, not
    /// errors: a torn write from a crashed process must not make the whole log
    /// unreadable.
    pub fn read_entries(&self) -> Vec<serde_json::Value> {
        let Ok(contents) = fs::read_to_string(self.dir.join(EVENTS_FILE)) else {
            return Vec::new();
        };
        contents
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
}
