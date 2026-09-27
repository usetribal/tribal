use std::io::{self, IsTerminal, Stdout};
use std::time::Instant;

use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use lineage_profiler::RatatuiReport;
use lineage_select::{
    draw_for_profile, Origin, Purpose, SearchError, SearchWorker, Selector, SessionMatch,
    SessionRow, SessionSearch,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

struct StubSearch;

impl SessionSearch for StubSearch {
    fn search(&self, _query: &str) -> Result<Vec<SessionMatch>, SearchError> {
        Ok(Vec::new())
    }
}

fn phase(
    name: &'static str,
    start: Instant,
    detail: Option<String>,
) -> lineage_profiler::RatatuiPhase {
    let micros = start.elapsed().as_micros();
    lineage_profiler::RatatuiPhase {
        name,
        millis: micros / 1000,
        micros,
        detail,
    }
}

pub fn synthetic_rows(count: usize) -> Vec<SessionRow> {
    let now = chrono::Utc::now();
    (0..count)
        .map(|i| SessionRow {
            id: format!("session-{i:04}"),
            title: format!("Synthetic session {i}"),
            agent: "cursor".into(),
            turns: 3,
            started_at: now,
            duration: None,
            project: Some("demo".into()),
            context: Some("opening ask line for width budget".into()),
            origin: Origin::Local,
            prompted_by: None,
        })
        .collect()
}

pub fn measure_selector_setup(rows: Vec<SessionRow>) -> RatatuiReport {
    let total = Instant::now();
    let mut phases = Vec::new();

    let t = Instant::now();
    let row_count = rows.len();
    let _selector = Selector::new(rows, Purpose::Browse);
    phases.push(phase("selector_new", t, Some(format!("rows={row_count}"))));

    let t = Instant::now();
    let _worker = SearchWorker::spawn(StubSearch);
    phases.push(phase("search_worker_spawn", t, None));

    RatatuiReport {
        total_millis: total.elapsed().as_millis(),
        phases,
        tty: false,
    }
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = crossterm::execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = crossterm::execute!(io::stdout(), LeaveAlternateScreen);
                Err(error)
            }
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

pub fn measure_terminal_first_frame(rows: Vec<SessionRow>) -> io::Result<RatatuiReport> {
    if !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "terminal profiling requires a TTY (try: script -q /dev/null …)",
        ));
    }

    let total = Instant::now();
    let mut phases = Vec::new();
    let row_count = rows.len();

    let t = Instant::now();
    let selector = Selector::new(rows, Purpose::Browse);
    phases.push(phase("selector_new", t, Some(format!("rows={row_count}"))));

    let t = Instant::now();
    let mut guard = TerminalGuard::enter()?;
    phases.push(phase("terminal_enter", t, None));

    let t = Instant::now();
    guard.terminal.draw(|frame| {
        draw_for_profile(frame, &selector, "lex");
    })?;
    phases.push(phase(
        "first_frame_draw",
        t,
        Some(format!("rows={row_count}")),
    ));

    Ok(RatatuiReport {
        total_millis: total.elapsed().as_millis(),
        phases,
        tty: true,
    })
}
