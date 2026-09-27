//! Headless timing harnesses. See `oss/harness/profiler/README.md`.

pub mod cli;
pub mod select;

pub use cli::tui_prep::measure_tui_prep;
pub use lineage_profiler::{CliPhase, PrepReport, RatatuiPhase, RatatuiReport};
