//! Progress bars for the long rebuild/backfill passes. Bars render on stderr
//! (indicatif's default) so JSON/stdout consumers stay clean, and stay hidden
//! when the corpus is empty (nothing to show progress for).

use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};

const STEADY_TICK: Duration = Duration::from_millis(120);

fn styled_bar(len: u64, verb: &str) -> ProgressBar {
    let bar = ProgressBar::new(len);
    // `unwrap` on a compile-time-constant template is fine: a bad template is a
    // programming error, not a runtime condition.
    bar.set_style(
        ProgressStyle::with_template(&format!(
            "{verb} {{pos}}/{{len}} [{{bar:30}}] {{elapsed_precise}} eta {{eta_precise}}"
        ))
        .unwrap()
        .progress_chars("=>-"),
    );
    bar.enable_steady_tick(STEADY_TICK);
    bar
}

/// A spinner with a running count for a phase whose total is unknown up front
/// (e.g. a git revwalk). Renders on stderr; `finish_and_clear` on drop-style
/// completion via [`Spinner::finish`].
pub struct Spinner {
    bar: ProgressBar,
}

impl Spinner {
    pub fn new(verb: &'static str) -> Self {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::with_template(&format!("{{spinner}} {verb} {{pos}} ({{elapsed}})"))
                .unwrap(),
        );
        bar.enable_steady_tick(STEADY_TICK);
        Self { bar }
    }

    pub fn set(&self, count: usize) {
        self.bar.set_position(count as u64);
    }

    pub fn finish(self) {
        self.bar.finish_and_clear();
    }
}

/// A progress bar driven by an `(done, total)` callback. The bar is created on
/// the first callback with `total > 0`, so an empty corpus renders nothing and
/// a non-empty one gets a live bar — creating it eagerly at length 0 would leave
/// it permanently hidden. Call [`SessionProgress::finish`] when the pass ends.
pub struct SessionProgress {
    verb: &'static str,
    bar: Option<ProgressBar>,
}

impl SessionProgress {
    pub fn new(verb: &'static str) -> Self {
        Self { verb, bar: None }
    }

    pub fn update(&mut self, done: usize, total: usize) {
        if total == 0 {
            return;
        }
        let bar = self
            .bar
            .get_or_insert_with(|| styled_bar(total as u64, self.verb));
        bar.set_length(total as u64);
        bar.set_position(done as u64);
    }

    pub fn finish(self) {
        if let Some(bar) = self.bar {
            bar.finish_and_clear();
        }
    }
}
