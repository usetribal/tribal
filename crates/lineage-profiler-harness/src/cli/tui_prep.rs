use std::path::Path;
use std::time::Instant;

use lineage_cli::flush::flush_and_collect_rows;
use lineage_cli::session_search::RepoSessionSearch;
use lineage_cli::session_transcript::load_session_entries;
use lineage_profiler::{CliPhase, PrepReport};

fn phase(name: &'static str, start: Instant, detail: Option<String>) -> CliPhase {
    CliPhase {
        name,
        millis: start.elapsed().as_millis(),
        detail,
    }
}

/// Blocking prep the interactive selector runs off-thread (harness baseline).
pub fn measure_tui_prep(repo_path: &Path, open_on: Option<&str>) -> PrepReport {
    let total_start = Instant::now();
    let mut phases = Vec::new();

    let t = Instant::now();
    let flush_detail = match flush_and_collect_rows(repo_path, &mut |_, _| {}) {
        Ok((flush, rows)) => Some(format!(
            "imported={} skipped={} failed={} sessions={}",
            flush.imported,
            flush.skipped,
            flush.failed,
            rows.len()
        )),
        Err(error) => Some(format!("error={error}")),
    };
    phases.push(phase("flush_and_collect_rows", t, flush_detail));

    let t = Instant::now();
    let search = RepoSessionSearch::open(repo_path).ok();
    phases.push(phase(
        "repo_session_search_open",
        t,
        search.as_ref().map(|s| format!("fused={}", s.is_fused())),
    ));

    if let Some(session_id) = open_on {
        let t = Instant::now();
        let entries = load_session_entries(repo_path, session_id)
            .map(|e| e.len())
            .unwrap_or(0);
        phases.push(phase(
            "load_session_entries",
            t,
            Some(format!("session={session_id} entries={entries}")),
        ));
    }

    PrepReport {
        total_millis: total_start.elapsed().as_millis(),
        phases,
    }
}
