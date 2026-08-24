//! The interactive loop: terminal setup, key translation, drawing.
//!
//! Deliberately thin. Everything it decides lives in [`Selector`] and
//! [`SearchWorker`], which are testable without a terminal; this translates
//! events into calls on them and draws what comes back.

use std::io::{self, Stdout};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use chrono::Utc;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding, Paragraph};
use ratatui::{Frame, Terminal};

use crate::confirm::{Choice, Confirm, Stage};
use crate::detail::{rendered_session_at, Match};
use crate::modal::{self, ModalStyles};
use crate::render::{row_lines, RowStyles, HORIZONTAL_MARGIN, LINES_PER_ROW};
use crate::search::SessionSearch;
use crate::session::{Purpose, SessionRow};
use crate::state::{Listing, Outcome, Screen, Selector};
use crate::transcript::Entry;
use crate::worker::SearchWorker;

/// How long typing must pause before a search runs. Long enough that a fast
/// typist causes one search rather than one per key, short enough to feel
/// immediate.
const DEBOUNCE: Duration = Duration::from_millis(300);
/// How long to block waiting for a key before redrawing. Bounds how late a
/// debounce fires or a search result appears.
const TICK: Duration = Duration::from_millis(50);
/// The frame interval while the modal is up. Short enough that the orb's motion
/// is smooth without spinning the loop when nothing is moving.
const ANIMATION_TICK: Duration = Duration::from_millis(33);

/// Restores the terminal however the loop ends.
///
/// A panic or an early return must not leave the user in raw mode on the
/// alternate screen, which looks like a hung shell.
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

/// Open the selector and block until the user confirms a session or backs out.
///
/// `leg` names the search that is available ("lex", "fused"), shown in the
/// corner: which one ran changes what a query will and will not find, and
/// leaving that invisible makes a thin result look like an empty repository.
///
/// `load` is called when a session is opened for reading, and returns its folded
/// transcript. The list holds no turns, so this is where one session's content
/// is read — on demand, for the one session someone asked to see.
pub fn select<S, L>(
    rows: Vec<SessionRow>,
    purpose: Purpose,
    search: S,
    leg: &str,
    load: L,
) -> io::Result<Outcome>
where
    S: SessionSearch + Send + 'static,
    L: Fn(&str) -> Vec<Entry>,
{
    select_with(rows, purpose, search, leg, load, |_| Ok(()))
}

/// As [`select`], with `share` performing the work the confirmation is for.
///
/// The share runs while the orb fills, so the animation covers real latency
/// rather than inventing a delay, and the burst plays only once it has
/// succeeded — a failure is never celebrated.
pub fn select_with<S, L, W>(
    rows: Vec<SessionRow>,
    purpose: Purpose,
    search: S,
    leg: &str,
    load: L,
    share: W,
) -> io::Result<Outcome>
where
    S: SessionSearch + Send + 'static,
    L: Fn(&str) -> Vec<Entry>,
    W: Fn(&str) -> Result<(), String> + Clone + Send + 'static,
{
    let leg = leg.to_string();
    let mut selector = Selector::new(rows, purpose);
    let mut worker = SearchWorker::spawn(search);
    let mut guard = TerminalGuard::enter()?;
    let mut pending_since: Option<Instant> = None;

    let mut page = 1usize;
    let mut rendered = 0usize;
    let mut matches: Vec<Match> = Vec::new();
    let mut confirm: Option<Confirm> = None;
    let mut shared: Option<String> = None;
    let mut failure: Option<String> = None;
    let mut work: Option<mpsc::Receiver<Result<String, String>>> = None;

    loop {
        guard.terminal.draw(|frame| {
            let frame_state = draw(frame, &selector, &leg);
            page = frame_state.viewport;
            rendered = frame_state.rendered;
            matches = frame_state.matches;
            if let Some(confirm) = confirm.as_ref() {
                modal::draw(frame, confirm, Instant::now(), &modal_styles());
            }
        })?;

        // The confirmation drives itself once it starts: the share runs under
        // the fill, the burst follows a success, and the modal closes when it
        // has played out.
        if let Some(pending) = confirm.as_mut() {
            let now = Instant::now();
            // A failure ends the modal immediately, checked before anything
            // else so the celebration can never outlive a share that did not
            // happen.
            if let Some(error) = failure.take() {
                return Err(io::Error::other(error));
            }
            // The share runs on its own thread, so the loop keeps drawing while
            // it works. Blocking here instead would stall on the network with
            // the clock already running, which is the dead pause the animation
            // exists to fill.
            if let Some(result) = work.as_ref().and_then(|rx| rx.try_recv().ok()) {
                match result {
                    Ok(id) => {
                        shared = Some(id);
                        pending.celebrate(now);
                    }
                    Err(error) => failure = Some(error),
                }
                work = None;
            }
            match pending.stage() {
                Stage::Working if shared.is_none() && work.is_none() && failure.is_none() => {
                    let id = selector
                        .confirmable()
                        .map(|row| row.id.clone())
                        .unwrap_or_default();
                    let (tx, rx) = mpsc::channel();
                    let job = share.clone();
                    std::thread::spawn(move || {
                        let _ = tx.send(job(&id).map(|()| id));
                    });
                    work = Some(rx);
                }
                Stage::Celebrating if pending.is_finished(now) => {
                    return Ok(Outcome::Chose(shared.unwrap_or_default()));
                }
                _ => {}
            }
            // Animating: pace the frames rather than spinning. Without the
            // wait the loop redraws as fast as the terminal allows, and the
            // whole animation is over in a fraction of its own duration.
            if pending.stage() != Stage::Asking {
                std::thread::sleep(ANIMATION_TICK);
                continue;
            }
        }

        // The opening match can only be placed once a render has said which
        // line it landed on, so it happens here rather than when the session
        // was opened.
        if let Some(line) = selector.take_pending_jump(&matches) {
            selector.scroll_to(line, page, rendered);
        }

        if let Some(answer) = worker.poll() {
            match answer.result {
                Ok(ids) => selector.apply_results(&ids),
                Err(error) => selector.apply_search_error(&error),
            }
        }

        if pending_since.is_some_and(|since| since.elapsed() >= DEBOUNCE) {
            pending_since = None;
            worker.request(selector.query());
        }

        // A shorter wait while the modal is up, so the orb breathes smoothly
        // instead of stepping.
        let wait = if confirm.is_some() {
            ANIMATION_TICK
        } else {
            TICK
        };
        if !event::poll(wait)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        // Ctrl-C leaves entirely from any screen; Esc is a step back, which on
        // the list is also leaving.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(selector.cancel());
        }

        if let Some(pending) = confirm.as_mut() {
            match key.code {
                KeyCode::Esc => confirm = None,
                KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l') => {
                    pending.move_focus()
                }
                KeyCode::Enter => match pending.answer() {
                    Choice::GoBack => confirm = None,
                    Choice::DoIt => pending.begin_work(Instant::now()),
                },
                _ => {}
            }
            continue;
        }

        // Typing inside a session goes to its own search, so the reading keys
        // (`j`, `k`, space) stay available until `/` asks for them as text.
        if selector.is_finding() {
            match key.code {
                KeyCode::Esc => selector.end_find(),
                // Enter walks the matches rather than closing the search: the
                // box stays open so the count keeps saying where you are.
                KeyCode::Enter => {
                    if let Some(line) = selector.next_match(&matches) {
                        selector.scroll_to(line, page, rendered);
                    }
                }
                KeyCode::Backspace => selector.pop_find_char(),
                KeyCode::Down => selector.scroll_by(1, page, rendered),
                KeyCode::Up => selector.scroll_by(-1, page, rendered),
                KeyCode::Char(c) => selector.push_find_char(c),
                _ => {}
            }
            continue;
        }

        if selector.is_reading() {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => selector.back(),
                KeyCode::Char('/') => selector.begin_find(),
                // Asks rather than sharing outright: enter is easy to press by
                // accident, and this is the irreversible step.
                KeyCode::Enter => {
                    if selector.confirmable().is_some() {
                        confirm = Some(Confirm::new());
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => selector.scroll_by(1, page, rendered),
                KeyCode::Up | KeyCode::Char('k') => selector.scroll_by(-1, page, rendered),
                KeyCode::PageDown | KeyCode::Char(' ') => {
                    selector.scroll_by(page as isize, page, rendered)
                }
                KeyCode::PageUp => selector.scroll_by(-(page as isize), page, rendered),
                _ => {}
            }
            continue;
        }

        match key.code {
            KeyCode::Esc => return Ok(selector.cancel()),
            KeyCode::Enter => {
                if let Some(row) = selector.open() {
                    let entries = load(&row.id);
                    selector.read(entries);
                }
            }
            KeyCode::Down => selector.move_down(),
            KeyCode::Up => selector.move_up(),
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                selector.move_down()
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                selector.move_up()
            }
            KeyCode::Backspace => {
                selector.pop_query_char();
                pending_since = (!selector.query().is_empty()).then(Instant::now);
            }
            KeyCode::Char(c) => {
                selector.push_query_char(c);
                pending_since = Some(Instant::now());
            }
            _ => {}
        }
    }
}

/// What one frame laid out, so the loop can bound scrolling and navigate
/// matches against what was actually drawn.
struct FrameState {
    viewport: usize,
    rendered: usize,
    matches: Vec<Match>,
}

fn draw(frame: &mut Frame, selector: &Selector, leg: &str) -> FrameState {
    // Inset from the terminal edges so the panel reads as a container rather
    // than text pinned to the frame.
    let page = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(HORIZONTAL_MARGIN),
            Constraint::Min(1),
            Constraint::Length(HORIZONTAL_MARGIN),
        ])
        .split(frame.area())[1];

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(page);

    let (rendered, matches, body_height) = match selector.screen() {
        Screen::Detail {
            entries, scroll, ..
        } => draw_detail(frame, selector, entries, *scroll, chunks[3]),
        Screen::List => {
            draw_list(frame, selector, chunks[3]);
            (0, Vec::new(), chunks[3].height as usize)
        }
    };
    // Drawn after the body, which is what counted the matches the box reports.
    draw_query(frame, selector, leg, &matches, chunks[1]);
    draw_hint(
        frame,
        selector.is_reading(),
        selector.is_finding(),
        chunks[4],
    );
    FrameState {
        // The body's height, not the whole chunk: the pinned header takes rows
        // the transcript never gets, and scrolling bounded by the larger number
        // would stop short of the last lines.
        viewport: body_height,
        rendered,
        matches,
    }
}

/// The session under consideration, scrolled to `scroll`. Returns how many lines
/// it holds, which of them carry a match, and the height the body was given.
fn draw_detail(
    frame: &mut Frame,
    selector: &Selector,
    entries: &[Entry],
    scroll: usize,
    area: Rect,
) -> (usize, Vec<Match>, usize) {
    let Some(row) = selector.reading_row() else {
        return (0, Vec::new(), area.height as usize);
    };
    let session = rendered_session_at(
        row,
        entries,
        area.width as usize,
        selector.find_query(),
        selector.current_match(),
        &styles(),
    );

    // The header is pinned above the body rather than scrolled with it, so
    // which session is open stays answerable from anywhere in a long transcript.
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(session.header.len() as u16),
            Constraint::Min(1),
        ])
        .split(area);
    frame.render_widget(Paragraph::new(session.header), split[0]);

    let total = session.lines.len();
    let visible: Vec<Line> = session.lines.into_iter().skip(scroll).collect();
    frame.render_widget(Paragraph::new(visible), split[1]);
    (total, session.matches, split[1].height as usize)
}

fn draw_query(frame: &mut Frame, selector: &Selector, leg: &str, matches: &[Match], area: Rect) {
    let title = match (selector.is_reading(), selector.purpose()) {
        (true, _) => " Search within the session ",
        (false, Purpose::Share) => " Choose a session to share ",
        (false, Purpose::Browse) => " Sessions ",
    };
    // The corner says where you are in whatever is being searched: results in
    // the list, matches inside a session.
    let position = if selector.is_reading() {
        match selector.match_position(matches) {
            (_, 0) if !selector.find_query().is_empty() => "no matches ".to_string(),
            (_, 0) => String::new(),
            (at, total) => format!("{at}/{total} matches "),
        }
    } else {
        match selector.selected_row() {
            Some(_) => format!(
                "{} {}/{} ",
                leg,
                selector.selected_position() + 1,
                selector.visible().len()
            ),
            None => format!("{leg} 0/0 "),
        }
    };
    // The box takes the accent while it is taking keystrokes, so a modal search
    // is visibly the thing with focus.
    let focused = selector.is_finding();
    let chrome = if focused { HOT } else { COOL };
    let query = Paragraph::new(Line::from(vec![
        Span::styled("❯ ", Style::default().fg(HOT)),
        Span::raw(if selector.is_reading() {
            selector.find_query().to_string()
        } else {
            selector.query().to_string()
        }),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(chrome))
            .padding(Padding::horizontal(1))
            .title(Span::styled(
                title,
                Style::default().fg(if focused { HOT } else { MID }),
            ))
            .title_top(
                Line::from(Span::styled(position, Style::default().fg(MID))).right_aligned(),
            ),
    );
    frame.render_widget(query, area);
}

fn draw_list(frame: &mut Frame, selector: &Selector, area: Rect) {
    if selector.is_empty() {
        frame.render_widget(empty_state(selector.listing()), area);
        return;
    }

    let styles = styles();
    let now = Utc::now();
    // Whole rows per page: a page break inside a session would split its header
    // from the context line that explains it.
    let per_page = (area.height as usize / LINES_PER_ROW).max(1);
    let offset = (selector.selected_position() / per_page) * per_page;

    let lines: Vec<Line> = selector
        .visible()
        .iter()
        .enumerate()
        .skip(offset)
        .take(per_page)
        .flat_map(|(position, &index)| {
            let row = &selector.rows()[index];
            let selected = position == selector.selected_position();
            let mut lines = row_lines(
                row,
                selector.purpose(),
                area.width as usize,
                now,
                selected,
                selector.query(),
                &styles,
            );
            if selected {
                lines = lines
                    .into_iter()
                    .map(|line| line.patch_style(styles.selected))
                    .collect();
            }
            lines
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), area);
}

/// The design system's empty-state pattern (`empty-states.md`), as a terminal
/// can express it: a mark in a bordered box, a sentence-case title stating the
/// absence, and a line describing what would appear. An empty view should
/// orient, not dead-end.
fn empty_state(listing: &Listing) -> Paragraph<'static> {
    // Told apart on purpose: a broken index must never read as "nothing
    // matched", which would send a user looking for a session that is there.
    let (mark, title, description, tone) = match listing {
        Listing::Failed(message) => ("!", "Search could not run", message.clone(), Color::Red),
        Listing::Searching => (
            "·",
            "Searching…",
            "Looking through what was said in each session.".to_string(),
            MID,
        ),
        Listing::Matched => (
            "○",
            "No sessions matched",
            "Try fewer words, or clear the search to see everything.".to_string(),
            MID,
        ),
        Listing::All => (
            "○",
            "No sessions here yet",
            "Sessions appear once an agent has worked in this repository.".to_string(),
            MID,
        ),
    };

    let boxed = Style::default().fg(COOL);
    Paragraph::new(vec![
        Line::default(),
        Line::from(Span::styled("╭───╮", boxed)).centered(),
        Line::from(vec![
            Span::styled("│ ", boxed),
            Span::styled(mark, Style::default().fg(tone)),
            Span::styled(" │", boxed),
        ])
        .centered(),
        Line::from(Span::styled("╰───╯", boxed)).centered(),
        Line::default(),
        Line::from(Span::styled(title, Style::default().fg(HOT))).centered(),
        Line::from(Span::styled(description, Style::default().fg(MID))).centered(),
    ])
}

fn draw_hint(frame: &mut Frame, reading: bool, finding: bool, area: Rect) {
    let keys = match (reading, finding) {
        (_, true) => "type to search · enter next match · esc done",
        (true, false) => "↑↓ scroll · / search · enter share · esc back",
        (false, false) => "type to search · ↑↓ move · enter open · esc cancel",
    };
    let hint = Paragraph::new(Line::from(Span::styled(keys, Style::default().fg(MID))));
    frame.render_widget(hint, area);
}

/// The accretion-disc palette the share page renders Gargantua with
/// (`apps/web/src/pages/share-page.tsx`), so the terminal and the web page a
/// share opens in read as the same product.
const HOT: Color = Color::Rgb(0xc8, 0xe0, 0xd4);
const MID: Color = Color::Rgb(0x5f, 0x8f, 0x78);
const COOL: Color = Color::Rgb(0x1e, 0x33, 0x29);

/// A fourth step between `MID` and `COOL`, for text that is secondary but still
/// has to be read.
///
/// The disc's three colours describe a gradient, not a legibility scale: `COOL`
/// is dark enough to sit against the background as a border, which is exactly
/// what makes it unreadable as prose. Without this step, "less important" and
/// "not meant to be read" are the same colour.
///
/// Chosen on the disc's own ramp for contrast rather than by eye: against a
/// typical dark terminal it lands near 3.4:1, above the 3:1 floor for secondary
/// text, while `MID` sits near 4.6:1 and stays visibly the more important of
/// the two. `COOL` is 1.3:1 — fine for a border, never for prose.
const FAINT: Color = Color::Rgb(0x4f, 0x77, 0x64);

/// The modal's palette, on the same disc ramp as everything else.
fn modal_styles() -> ModalStyles {
    ModalStyles {
        border: Style::default().fg(MID),
        title: Style::default().fg(HOT).add_modifier(Modifier::BOLD),
        focused: Style::default().fg(HOT),
        unfocused: Style::default().fg(FAINT),
        orb: Style::default().fg(MID),
    }
}

fn styles() -> RowStyles {
    RowStyles {
        project: Style::default().add_modifier(Modifier::BOLD),
        title: Style::default().fg(HOT),
        meta: Style::default().fg(MID),
        context: Style::default().fg(MID),
        // White for who spoke, disc green for what they said, the faint step for
        // what it did: three ranks a reader can tell apart at a glance.
        speaker: Style::default().fg(Color::White),
        // Two distinct jobs, previously one colour: chrome that should recede,
        // and prose that is secondary but still has to be read.
        recessive: Style::default().fg(COOL),
        faint: Style::default().fg(FAINT),
        accent: Style::default().fg(HOT),
        border: Style::default().fg(COOL),
        // Brighter than the prose it sits in, not the same green: context is
        // MID now, so a MID hit reads as bold-only and the match disappears.
        hit: Style::default().fg(HOT).add_modifier(Modifier::BOLD),
        // A left bar rather than a reversed block: reversing a two-line row
        // paints a slab across the list and hides the styling inside it.
        selected: Style::default(),
    }
}
