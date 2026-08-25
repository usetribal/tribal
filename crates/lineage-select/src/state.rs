//! What the selector is showing and what a keypress does to it.
//!
//! Separate from the terminal so the whole interaction is testable without one:
//! the loop in `tui` only translates events into these calls and draws the
//! result.

use crate::detail::Match;
use crate::search::{SearchError, SessionMatch};
use crate::session::{Purpose, SessionRow};
use crate::transcript::Entry;

/// What a keypress asked the selector to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Stay open.
    Continue,
    /// The user chose this session id.
    Chose(String),
    /// The user backed out without choosing.
    Cancelled,
}

/// Which screen the selector is showing.
///
/// Choosing is two steps rather than a yes/no gate: someone who opens a session
/// and finds it is the wrong one wants to go back to choosing, not to cancel
/// the whole thing.
pub enum Screen {
    /// Scanning the list.
    List,
    /// Reading one session before committing to it.
    Detail {
        /// Index into `rows` of the session being read.
        row: usize,
        entries: Vec<Entry>,
        /// First visible line of the transcript.
        scroll: usize,
        /// Text being searched for within this session. Separate from the list's
        /// query: one finds a session, the other finds a passage inside it, and
        /// sharing a field would make leaving the pane clear the wrong one.
        find: String,
        /// Whether the search box is taking keystrokes. Distinct from `find`
        /// having text: leaving the box keeps what was typed and its
        /// highlighting, and only stops the box swallowing the reading keys.
        finding: bool,
        /// Which match the reader is on, into the rendered match list.
        at_match: usize,
        /// Set until the first draw has placed the opening match. The line a
        /// match lands on is only known once the session has been rendered, so
        /// the jump happens on the first frame rather than here.
        pending_jump: bool,
    },
}

/// Where the visible list came from, so the empty case can say something true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    /// No query: every session, newest first.
    All,
    /// Filtered by a query that ran.
    Matched,
    /// A query that could not run. Kept distinct from an empty match so a
    /// broken index never reads as "nothing matched".
    Failed(String),
    /// A query is typed but its result has not arrived.
    Searching,
}

pub struct Selector {
    rows: Vec<SessionRow>,
    /// Each row's context before any query touched it, so clearing the query
    /// restores what the row said rather than leaving a stale passage.
    openings: Vec<Option<String>>,
    purpose: Purpose,
    /// Indices into `rows`, in display order.
    visible: Vec<usize>,
    selected: usize,
    query: String,
    listing: Listing,
    screen: Screen,
}

impl Selector {
    pub fn new(rows: Vec<SessionRow>, purpose: Purpose) -> Self {
        let visible = (0..rows.len()).collect();
        let openings = rows.iter().map(|row| row.context.clone()).collect();
        let mut selector = Self {
            rows,
            openings,
            purpose,
            visible,
            selected: 0,
            query: String::new(),
            listing: Listing::All,
            screen: Screen::List,
        };
        selector.settle_selection(0);
        selector
    }

    pub fn rows(&self) -> &[SessionRow] {
        &self.rows
    }

    pub fn visible(&self) -> &[usize] {
        &self.visible
    }

    pub fn purpose(&self) -> Purpose {
        self.purpose
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn listing(&self) -> &Listing {
        &self.listing
    }

    pub fn selected_position(&self) -> usize {
        self.selected
    }

    pub fn selected_row(&self) -> Option<&SessionRow> {
        self.visible.get(self.selected).map(|&i| &self.rows[i])
    }

    pub fn is_empty(&self) -> bool {
        self.visible.is_empty()
    }

    /// Move the cursor to a session by id, so a caller that already knows which
    /// session it wants can open the selector on it.
    ///
    /// Returns whether the id was found. A caller that named a session the list
    /// does not hold gets the list rather than an error: the session still
    /// exists as far as the user is concerned, and the list is where they would
    /// go looking for it.
    pub fn focus(&mut self, session_id: &str) -> bool {
        let Some(position) = self
            .visible
            .iter()
            .position(|&i| self.rows[i].id == session_id)
        else {
            return false;
        };
        self.selected = position;
        true
    }

    pub fn push_query_char(&mut self, c: char) {
        self.query.push(c);
        self.listing = Listing::Searching;
    }

    pub fn pop_query_char(&mut self) {
        self.query.pop();
        if self.query.is_empty() {
            self.show_all();
            return;
        }
        self.listing = Listing::Searching;
    }

    /// Restore the unfiltered list — the empty query means "everything", so no
    /// search runs for it.
    pub fn show_all(&mut self) {
        // Matched passages belong to the query that found them, so clearing it
        // puts each row's own opening back.
        for (index, row) in self.rows.iter_mut().enumerate() {
            row.context.clone_from(&self.openings[index]);
        }
        self.visible = (0..self.rows.len()).collect();
        self.listing = Listing::All;
        self.settle_selection(0);
    }

    /// Apply a completed search. Ids the list does not hold are ignored: the
    /// index can outlive a session that has since gone.
    ///
    /// A match that came with a passage replaces that row's context line, so
    /// the row shows why it matched rather than how it opened.
    pub fn apply_results(&mut self, matches: &[SessionMatch]) {
        self.visible = matches
            .iter()
            .filter_map(|found| {
                let index = self.rows.iter().position(|row| row.id == found.id)?;
                if let Some(passage) = found.passage.as_deref() {
                    self.rows[index].context = Some(passage.to_string());
                }
                Some(index)
            })
            .collect();
        self.listing = Listing::Matched;
        self.settle_selection(0);
    }

    pub fn apply_search_error(&mut self, error: &SearchError) {
        self.visible.clear();
        self.listing = Listing::Failed(error.message.clone());
        self.selected = 0;
    }

    pub fn move_down(&mut self) {
        if self.visible.is_empty() {
            return;
        }
        let next = (self.selected + 1).min(self.visible.len() - 1);
        self.settle_selection_from(next, 1);
    }

    pub fn move_up(&mut self) {
        if self.visible.is_empty() {
            return;
        }
        let next = self.selected.saturating_sub(1);
        self.settle_selection_from(next, -1);
    }

    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    pub fn is_reading(&self) -> bool {
        matches!(self.screen, Screen::Detail { .. })
    }

    /// The row the detail screen is showing.
    pub fn reading_row(&self) -> Option<&SessionRow> {
        match &self.screen {
            Screen::Detail { row, .. } => self.rows.get(*row),
            Screen::List => None,
        }
    }

    /// The session the user wants to look at, if the highlighted one can be
    /// chosen at all. The caller loads its turns and calls [`Self::read`].
    ///
    /// An ineligible row opens nothing: it is shown so the user can see the
    /// session exists and why it is unavailable, not so it can be picked and
    /// refused a step later.
    pub fn open(&self) -> Option<&SessionRow> {
        self.selected_row()
            .filter(|row| self.purpose.eligibility(row).is_none())
    }

    /// Show the loaded session.
    pub fn read(&mut self, entries: Vec<Entry>) {
        let Some(&row) = self.visible.get(self.selected) else {
            return;
        };
        self.screen = Screen::Detail {
            row,
            entries,
            scroll: 0,
            // Opens searching for whatever the list was searching for, so the
            // passage the row previewed is the passage the session opens on.
            find: self.query.clone(),
            finding: false,
            at_match: 0,
            pending_jump: true,
        };
    }

    /// Leave the detail screen for the list, keeping the query and selection —
    /// backing out of a session returns to choosing, not to nothing.
    pub fn back(&mut self) {
        self.screen = Screen::List;
    }

    /// Give the search box focus, keeping whatever it already holds.
    pub fn begin_find(&mut self) {
        if let Screen::Detail { finding, .. } = &mut self.screen {
            *finding = true;
        }
    }

    /// Move to the next occurrence, wrapping at the end. Returns its index.
    pub fn next_match(&mut self, matches: &[Match]) -> Option<usize> {
        let Screen::Detail { at_match, .. } = &mut self.screen else {
            return None;
        };
        if matches.is_empty() {
            return None;
        }
        *at_match = (*at_match + 1) % matches.len();
        matches.get(*at_match).map(|found| found.line)
    }

    /// Which match the reader is on, as an index into the rendered match list.
    pub fn current_match(&self) -> Option<usize> {
        match &self.screen {
            Screen::Detail { at_match, .. } => Some(*at_match),
            Screen::List => None,
        }
    }

    /// Which match the reader is on, one-based, and how many there are.
    pub fn match_position(&self, matches: &[Match]) -> (usize, usize) {
        match &self.screen {
            Screen::Detail { at_match, .. } if !matches.is_empty() => {
                ((*at_match % matches.len()) + 1, matches.len())
            }
            _ => (0, matches.len()),
        }
    }

    /// Place the view on the opening match, once the render has said where it
    /// is. Does nothing after the first time, so scrolling is not undone by the
    /// next frame.
    pub fn take_pending_jump(&mut self, matches: &[Match]) -> Option<usize> {
        let Screen::Detail {
            pending_jump,
            at_match,
            ..
        } = &mut self.screen
        else {
            return None;
        };
        if !*pending_jump {
            return None;
        }
        *pending_jump = false;
        *at_match = 0;
        matches.first().map(|found| found.line)
    }

    /// Scroll so `line` is at the top of the viewport, clamped to the content.
    pub fn scroll_to(&mut self, line: usize, viewport: usize, rendered: usize) {
        let Screen::Detail { scroll, .. } = &mut self.screen else {
            return;
        };
        // A little context above the match, so it does not sit flush against
        // the top edge with its speaker label scrolled off.
        let target = line.saturating_sub(2);
        *scroll = target.min(rendered.saturating_sub(viewport));
    }

    /// Stop searching, keeping the session open.
    /// Take focus off the search box.
    ///
    /// The query and its highlighting stay: leaving the box is done searching,
    /// not undoing the search, and clearing it here would throw away what the
    /// reader just typed.
    pub fn end_find(&mut self) {
        if let Screen::Detail { finding, .. } = &mut self.screen {
            *finding = false;
        }
    }

    pub fn is_finding(&self) -> bool {
        matches!(self.screen, Screen::Detail { finding: true, .. })
    }

    /// What is being searched for inside the session.
    pub fn find_query(&self) -> &str {
        match &self.screen {
            Screen::Detail { find, .. } => find,
            Screen::List => "",
        }
    }

    pub fn push_find_char(&mut self, c: char) {
        if let Screen::Detail { find, at_match, .. } = &mut self.screen {
            find.push(c);
            *at_match = 0;
        }
    }

    pub fn pop_find_char(&mut self) {
        if let Screen::Detail { find, at_match, .. } = &mut self.screen {
            find.pop();
            *at_match = 0;
        }
    }

    pub fn scroll_by(&mut self, delta: isize, viewport: usize, rendered: usize) {
        let Screen::Detail { scroll, .. } = &mut self.screen else {
            return;
        };
        let furthest = rendered.saturating_sub(viewport);
        let next = (*scroll as isize + delta).max(0) as usize;
        *scroll = next.min(furthest);
    }

    /// The session being read, if it may actually be shared. What the
    /// confirmation is about.
    pub fn confirmable(&self) -> Option<&SessionRow> {
        self.reading_row()
            .filter(|row| self.purpose.eligibility(row).is_none())
    }

    /// Commit to the session being read. Only the detail screen can confirm, so
    /// nothing is shared without having been looked at.
    pub fn confirm(&self) -> Outcome {
        match self.reading_row() {
            Some(row) if self.purpose.eligibility(row).is_none() => Outcome::Chose(row.id.clone()),
            _ => Outcome::Continue,
        }
    }

    pub fn cancel(&self) -> Outcome {
        Outcome::Cancelled
    }

    fn settle_selection(&mut self, from: usize) {
        self.settle_selection_from(from, 1);
    }

    /// Land on a choosable row, searching in `step` direction and falling back
    /// the other way. An all-ineligible list still has to put the cursor
    /// somewhere, so it settles on `from`.
    fn settle_selection_from(&mut self, from: usize, step: isize) {
        if self.visible.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.visible.len();
        let from = from.min(len - 1);
        if let Some(found) = self.scan(from, step, len) {
            self.selected = found;
            return;
        }
        if let Some(found) = self.scan(from, -step, len) {
            self.selected = found;
            return;
        }
        self.selected = from;
    }

    fn scan(&self, from: usize, step: isize, len: usize) -> Option<usize> {
        let mut at = from as isize;
        while at >= 0 && (at as usize) < len {
            let row = &self.rows[self.visible[at as usize]];
            if self.purpose.eligibility(row).is_none() {
                return Some(at as usize);
            }
            at += step;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Origin;
    use chrono::{DateTime, TimeZone, Utc};

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, day, 10, 0, 0).unwrap()
    }

    fn row(id: &str, origin: Origin) -> SessionRow {
        SessionRow {
            id: id.into(),
            title: format!("Session {id}"),
            agent: "claude".into(),
            turns: 3,
            started_at: at(20),
            duration: None,
            project: Some("acme-app".into()),
            origin,
            prompted_by: None,
            context: None,
        }
    }

    fn local_rows() -> Vec<SessionRow> {
        vec![
            row("a", Origin::Local),
            row("b", Origin::Local),
            row("c", Origin::Local),
        ]
    }

    #[test]
    fn a_new_selector_shows_everything_and_selects_the_first() {
        let selector = Selector::new(local_rows(), Purpose::Share);
        assert_eq!(selector.visible().len(), 3);
        assert_eq!(selector.selected_row().map(|r| r.id.as_str()), Some("a"));
        assert_eq!(selector.listing(), &Listing::All);
    }

    #[test]
    fn opening_then_confirming_returns_the_selected_id() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.move_down();
        assert_eq!(selector.open().map(|r| r.id.as_str()), Some("b"));
        selector.read(vec![]);
        assert_eq!(selector.confirm(), Outcome::Chose("b".into()));
    }

    #[test]
    fn nothing_is_confirmed_without_being_opened_first() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        assert_eq!(
            selector.confirm(),
            Outcome::Continue,
            "the list screen cannot share"
        );
        selector.read(vec![]);
        assert_eq!(selector.confirm(), Outcome::Chose("a".into()));
    }

    #[test]
    fn backing_out_of_a_session_returns_to_choosing() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.move_down();
        selector.read(vec![]);
        assert!(selector.is_reading());

        selector.back();
        assert!(!selector.is_reading());
        // The list is where it was left, so backing out costs nothing.
        assert_eq!(selector.selected_row().map(|r| r.id.as_str()), Some("b"));
    }

    #[test]
    fn an_ineligible_row_cannot_be_opened() {
        let rows = vec![row("received", Origin::Received)];
        let selector = Selector::new(rows, Purpose::Share);
        assert!(selector.open().is_none());
    }

    #[test]
    fn the_cursor_skips_past_ineligible_rows() {
        let rows = vec![
            row("a", Origin::Local),
            row("received", Origin::Received),
            row("c", Origin::Local),
        ];
        let mut selector = Selector::new(rows, Purpose::Share);
        selector.move_down();
        assert_eq!(selector.selected_row().map(|r| r.id.as_str()), Some("c"));
    }

    #[test]
    fn the_first_choosable_row_is_selected_when_the_list_opens_on_an_ineligible_one() {
        let rows = vec![row("received", Origin::Received), row("b", Origin::Local)];
        let selector = Selector::new(rows, Purpose::Share);
        assert_eq!(selector.selected_row().map(|r| r.id.as_str()), Some("b"));
    }

    #[test]
    fn browsing_can_select_a_received_session() {
        let rows = vec![row("received", Origin::Received)];
        let mut selector = Selector::new(rows, Purpose::Browse);
        assert!(selector.open().is_some());
        selector.read(vec![]);
        assert_eq!(selector.confirm(), Outcome::Chose("received".into()));
    }

    #[test]
    fn results_reorder_the_list_into_relevance_order() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.apply_results(&["c".into(), "a".into()]);
        let ids: Vec<&str> = selector
            .visible()
            .iter()
            .map(|&i| selector.rows()[i].id.as_str())
            .collect();
        assert_eq!(ids, vec!["c", "a"]);
    }

    #[test]
    fn results_naming_an_unknown_session_ignore_it() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.apply_results(&["gone".into(), "b".into()]);
        assert_eq!(selector.visible().len(), 1);
        assert_eq!(selector.selected_row().map(|r| r.id.as_str()), Some("b"));
    }

    #[test]
    fn a_failed_search_is_not_an_empty_result() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.apply_search_error(&SearchError::new("index missing"));
        assert!(selector.is_empty());
        assert_eq!(selector.listing(), &Listing::Failed("index missing".into()));
    }

    #[test]
    fn clearing_the_query_restores_every_session() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.apply_results(&["c".into()]);
        selector.push_query_char('x');
        selector.pop_query_char();
        assert_eq!(selector.visible().len(), 3);
        assert_eq!(selector.listing(), &Listing::All);
    }

    #[test]
    fn choosing_nothing_from_an_empty_list_stays_open() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.apply_results(&[]);
        assert!(selector.open().is_none());
        assert_eq!(selector.cancel(), Outcome::Cancelled);
    }

    #[test]
    fn searching_within_a_session_is_separate_from_the_list_search() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.push_query_char('a');
        selector.read(vec![]);

        // The session opens searching for what the list searched for, so the
        // previewed passage is where the reader lands.
        assert_eq!(selector.find_query(), "a");

        selector.begin_find();
        selector.pop_find_char();
        selector.push_find_char('x');
        assert_eq!(selector.find_query(), "x");
        // The list's own query is untouched, so backing out lands where the
        // user left off rather than on a list they never searched for.
        assert_eq!(selector.query(), "a");

        selector.end_find();
        assert!(!selector.is_finding());
        assert!(
            selector.is_reading(),
            "leaving the search keeps the session"
        );
    }

    #[test]
    fn a_find_query_does_not_survive_reopening_a_session() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.read(vec![]);
        selector.begin_find();
        selector.push_find_char('x');

        selector.back();
        selector.read(vec![]);
        // Reopened with no list query, so nothing is being searched for.
        assert_eq!(selector.find_query(), "");
    }

    #[test]
    fn enter_walks_the_matches_and_wraps() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.read(vec![]);
        let matches = [
            Match { line: 4, span: 1 },
            Match { line: 20, span: 3 },
            Match { line: 51, span: 0 },
        ];

        assert_eq!(selector.match_position(&matches), (1, 3));
        assert_eq!(selector.next_match(&matches), Some(20));
        assert_eq!(selector.match_position(&matches), (2, 3));
        assert_eq!(selector.next_match(&matches), Some(51));
        assert_eq!(selector.next_match(&matches), Some(4), "wraps to the first");
        assert_eq!(selector.match_position(&matches), (1, 3));
    }

    #[test]
    fn a_session_with_no_matches_reports_none_and_does_not_move() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.read(vec![]);
        assert_eq!(selector.next_match(&[]), None);
        assert_eq!(selector.match_position(&[]), (0, 0));
    }

    #[test]
    fn the_opening_jump_lands_on_the_first_match_and_happens_once() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.push_query_char('a');
        selector.read(vec![]);
        let matches = [Match { line: 30, span: 2 }, Match { line: 60, span: 1 }];

        assert_eq!(selector.take_pending_jump(&matches), Some(30));
        // Only once: a later frame must not drag the view back off wherever the
        // reader has since scrolled to.
        assert_eq!(selector.take_pending_jump(&matches), None);
    }

    #[test]
    fn editing_the_query_restarts_the_match_cycle() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.read(vec![]);
        selector.begin_find();
        let matches = [
            Match { line: 1, span: 0 },
            Match { line: 2, span: 0 },
            Match { line: 3, span: 0 },
        ];
        selector.next_match(&matches);
        assert_eq!(selector.match_position(&matches), (2, 3));

        selector.push_find_char('z');
        assert_eq!(
            selector.match_position(&matches),
            (1, 3),
            "a changed query has different matches, so the cursor resets"
        );
    }

    #[test]
    fn scrolling_to_a_match_keeps_context_above_it() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.read(vec![]);
        selector.scroll_to(40, 10, 100);
        assert!(matches!(
            selector.screen(),
            Screen::Detail { scroll: 38, .. }
        ));
    }

    #[test]
    fn leaving_the_search_box_keeps_what_was_typed() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.push_query_char('a');
        selector.read(vec![]);

        selector.begin_find();
        selector.push_find_char('z');
        assert_eq!(selector.find_query(), "az");

        selector.end_find();
        // Done searching, not undoing the search: the text and its highlighting
        // stay, and only the box's focus is given up.
        assert!(!selector.is_finding());
        assert_eq!(selector.find_query(), "az");
    }

    #[test]
    fn only_a_session_being_read_can_be_searched_within() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.begin_find();
        assert!(!selector.is_finding(), "the list has its own search");
        assert_eq!(selector.find_query(), "");
    }

    #[test]
    fn scrolling_stops_at_both_ends_of_the_transcript() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.read(vec![]);

        selector.scroll_by(-5, 10, 40);
        assert!(matches!(
            selector.screen(),
            Screen::Detail { scroll: 0, .. }
        ));

        selector.scroll_by(1000, 10, 40);
        // The last page stays full rather than scrolling into blank space.
        assert!(matches!(
            selector.screen(),
            Screen::Detail { scroll: 30, .. }
        ));
    }

    #[test]
    fn a_transcript_shorter_than_the_viewport_does_not_scroll() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.read(vec![]);
        selector.scroll_by(5, 40, 10);
        assert!(matches!(
            selector.screen(),
            Screen::Detail { scroll: 0, .. }
        ));
    }

    #[test]
    fn moving_stays_in_range_at_both_ends() {
        let mut selector = Selector::new(local_rows(), Purpose::Share);
        selector.move_up();
        assert_eq!(selector.selected_position(), 0);
        for _ in 0..10 {
            selector.move_down();
        }
        assert_eq!(selector.selected_position(), 2);
    }

    /// How `show <id>` opens on a session: the cursor moves to it, and reading
    /// leaves the whole list behind so backing out returns to browsing.
    #[test]
    fn focusing_a_session_moves_the_cursor_and_keeps_the_list_behind_it() {
        let mut selector = Selector::new(
            vec![
                row("a", Origin::Local),
                row("b", Origin::Local),
                row("c", Origin::Local),
            ],
            Purpose::Browse,
        );

        assert!(selector.focus("c"));
        assert_eq!(selector.selected_row().map(|r| r.id.as_str()), Some("c"));

        selector.read(vec![]);
        assert!(selector.is_reading());

        selector.back();
        assert!(!selector.is_reading());
        assert_eq!(
            selector.rows().len(),
            3,
            "the list is still there to return to"
        );
    }

    /// A session the list does not hold leaves the cursor alone rather than
    /// failing: the caller opens on the list, which is where someone would go
    /// looking for it anyway.
    #[test]
    fn focusing_an_unknown_session_reports_it_and_changes_nothing() {
        let mut selector = Selector::new(
            vec![row("a", Origin::Local), row("b", Origin::Local)],
            Purpose::Browse,
        );
        let before = selector.selected_position();

        assert!(!selector.focus("missing"));
        assert_eq!(selector.selected_position(), before);
    }
}
