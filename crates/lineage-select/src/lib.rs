//! Terminal session selector.
//!
//! Renders a list of sessions, filters it by what was said in them, opens one
//! for reading, and returns the one confirmed. It knows nothing about git,
//! imports, or what the caller will do with the result: a caller assembles
//! [`SessionRow`]s, names its [`Purpose`], supplies a [`SessionSearch`], and
//! loads one session's [`Entry`] list when asked. Rendering is exported apart
//! from the interactive loop, so a non-interactive caller can print the same
//! rows ([`row_lines`]) or the same session ([`session_lines`]).

mod confirm;
mod detail;
mod modal;
mod orb;
mod render;
mod search;
mod session;
mod state;
mod transcript;
mod tui;
mod worker;

pub use confirm::{Choice, Confirm, Stage};
pub use detail::{rendered_session, rendered_session_at, session_lines, Match, RenderedSession};
pub use modal::ModalStyles;
pub use render::{
    boxed_block, detail_lines, row_lines, RowStyles, HORIZONTAL_MARGIN, LINES_PER_ROW,
};
pub use search::{SearchError, SessionMatch, SessionSearch};
pub use session::{Ineligible, Origin, Purpose, SessionRow};
pub use state::{Listing, Outcome, Screen, Selector};
pub use transcript::{
    activity_duration, activity_summary, activity_tools, fold, Entry, Speaker, TranscriptTurn,
};
pub use tui::{select, select_opening_on, select_with};
pub use worker::{Answer, SearchWorker};
