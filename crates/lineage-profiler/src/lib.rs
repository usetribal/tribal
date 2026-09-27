//! Runtime profiling hooks shared by CLI and selector crates (dev `profile` feature only).
//!
//! Headless measurement lives in `lineage-profiler-harness` (`publish = false`, not linked
//! into release `tribal`). This crate is env detection and JSON lines on stderr when
//! `lineage-cli` / `lineage-select` are built with `--features profile`.

mod env;
mod report;

pub use env::{profile_repo_from_env, profiling_enabled};
pub use report::{
    emit_prep, emit_ratatui, phase_from_elapsed, CliPhase, PrepReport, RatatuiPhase, RatatuiReport,
};
