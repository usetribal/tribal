//! CLI selector prep + ratatui micro-benchmarks. See `oss/harness/profiler/README.md`.

use std::path::Path;

use lineage_profiler::{profile_repo_from_env, PrepReport, RatatuiReport};
use lineage_profiler_harness::cli::tui_prep::measure_tui_prep;
use lineage_profiler_harness::select::{
    measure_selector_setup, measure_terminal_first_frame, synthetic_rows,
};

fn print_ratatui(label: &str, report: &RatatuiReport) {
    eprintln!("=== {label} (ratatui) tty={} ===", report.tty);
    for phase in &report.phases {
        let detail = phase.detail.as_deref().unwrap_or("");
        eprintln!(
            "  {:28} {:>6.3} ms ({:>5} µs)  {detail}",
            phase.name,
            phase.micros as f64 / 1000.0,
            phase.micros
        );
    }
    eprintln!("  {:28} {:>6} ms", "total", report.total_millis);
}

fn print_prep(label: &str, report: &PrepReport) {
    eprintln!("=== {label} ===");
    for phase in &report.phases {
        let detail = phase.detail.as_deref().unwrap_or("");
        eprintln!("  {:28} {:>6} ms  {detail}", phase.name, phase.millis);
    }
    eprintln!("  {:28} {:>6} ms", "total", report.total_millis);
}

#[test]
#[ignore = "local profiling harness; set TRIBAL_TUI_PROFILE_REPO or run from oss monorepo parent"]
fn tui_prep_breakdown_list_path() {
    let repo = profile_repo_from_env(Path::new(env!("CARGO_MANIFEST_DIR")));
    assert!(
        repo.join(".git").exists(),
        "repo missing .git: {}",
        repo.display()
    );
    print_prep("list (browse prep)", &measure_tui_prep(&repo, None));
}

#[test]
#[ignore = "local profiling harness"]
fn ratatui_in_process_setup_scales_with_row_count() {
    for count in [1usize, 50, 392] {
        let report = measure_selector_setup(synthetic_rows(count));
        print_ratatui(&format!("selector setup (synthetic rows={count})"), &report);
    }
}

#[test]
#[ignore = "requires TTY — run: script -q /dev/null cargo test -p lineage-profiler-harness ratatui_terminal -- --ignored --nocapture"]
fn ratatui_terminal_first_frame() {
    for count in [1usize, 392] {
        let rows = synthetic_rows(count);
        match measure_terminal_first_frame(rows) {
            Ok(report) => {
                print_ratatui(&format!("terminal first frame (rows={count})"), &report);
            }
            Err(error) => {
                eprintln!("skip rows={count}: {error}");
            }
        }
    }
}

#[test]
#[ignore = "local profiling harness"]
fn tui_prep_breakdown_warm_second_run() {
    let repo = profile_repo_from_env(Path::new(env!("CARGO_MANIFEST_DIR")));
    let _first = measure_tui_prep(&repo, None);
    print_prep("list prep (warm)", &measure_tui_prep(&repo, None));
}
