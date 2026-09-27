//! Ratatui loop timing — only compiled when the `profile` feature is enabled.

use std::time::Instant;

#[cfg(feature = "profile")]
use lineage_profiler::{emit_ratatui, phase_from_elapsed, profiling_enabled, RatatuiReport};

/// Collects ratatui startup phases for `TRIBAL_TUI_PROFILE=1` (dev builds only).
pub(crate) struct RatatuiProfiler {
    #[cfg(feature = "profile")]
    state: Option<(Instant, Vec<lineage_profiler::RatatuiPhase>)>,
    #[cfg(feature = "profile")]
    emitted: bool,
}

impl RatatuiProfiler {
    pub fn begin() -> Self {
        #[cfg(feature = "profile")]
        {
            Self {
                state: profiling_enabled().then(|| (Instant::now(), Vec::new())),
                emitted: false,
            }
        }
        #[cfg(not(feature = "profile"))]
        {
            Self {}
        }
    }

    pub fn on_worker_spawn(&mut self, start: Instant) {
        #[cfg(not(feature = "profile"))]
        let _ = start;
        #[cfg(feature = "profile")]
        if let Some((_, phases)) = &mut self.state {
            phases.push(phase_from_elapsed(
                "search_worker_spawn",
                start.elapsed(),
                None,
            ));
        }
    }

    pub fn on_terminal_enter(&mut self, start: Instant) {
        #[cfg(not(feature = "profile"))]
        let _ = start;
        #[cfg(feature = "profile")]
        if let Some((_, phases)) = &mut self.state {
            phases.push(phase_from_elapsed("terminal_enter", start.elapsed(), None));
        }
    }

    pub fn worker_spawn_start(&self) -> Option<Instant> {
        #[cfg(feature = "profile")]
        {
            self.state.as_ref().map(|_| Instant::now())
        }
        #[cfg(not(feature = "profile"))]
        {
            None
        }
    }

    pub fn terminal_enter_start(&self) -> Option<Instant> {
        self.worker_spawn_start()
    }

    pub fn first_frame_draw_start(&self) -> Option<Instant> {
        #[cfg(feature = "profile")]
        {
            (!self.emitted)
                .then_some(self.state.as_ref().map(|_| Instant::now()))
                .flatten()
        }
        #[cfg(not(feature = "profile"))]
        {
            None
        }
    }

    pub fn on_first_frame_draw(&mut self, start: Instant, row_count: usize) {
        #[cfg(not(feature = "profile"))]
        {
            let _ = (start, row_count);
        }
        #[cfg(feature = "profile")]
        if let Some((total_start, phases)) = &mut self.state {
            phases.push(phase_from_elapsed(
                "first_frame_draw",
                start.elapsed(),
                Some(format!("rows={row_count}")),
            ));
            emit_ratatui(&RatatuiReport {
                total_millis: total_start.elapsed().as_millis(),
                phases: phases.clone(),
                tty: true,
            });
            self.emitted = true;
        }
    }
}
