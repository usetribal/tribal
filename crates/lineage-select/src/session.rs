//! The minimal session shape the selector renders, and what a caller intends to
//! do with the chosen one.

use chrono::{DateTime, Duration, Utc};

/// Where a session came from, as far as the selector needs to care.
///
/// This is deliberately coarser than the provenance model in `lineage-core`:
/// the selector decides how to *show* a session, so it only needs the
/// distinctions that change what a user may do with one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Imported on this machine from a local agent transcript.
    Local,
    /// Pulled from a lineage server — the server that holds it is its source of
    /// truth, so this machine cannot push it anywhere.
    Received,
}

/// One row of the selector.
///
/// A caller assembles these from whatever it has (stored conversations, a
/// server listing) and the selector never reaches back for more. Everything
/// needed to render and rank a row is here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub agent: String,
    pub turns: usize,
    pub started_at: DateTime<Utc>,
    /// How long the session ran, when its end is known. A session still being
    /// written has no end yet.
    pub duration: Option<Duration>,
    /// The project the session belongs to, as a leading label. Sessions group
    /// by project before anything else, so this reads first on the row.
    pub project: Option<String>,
    pub origin: Origin,
    /// Display name of whoever prompted the session, when known.
    pub prompted_by: Option<String>,
    /// A line of the session's own text for the row's second line: the matched
    /// passage once a query has run, and its opening otherwise.
    pub context: Option<String>,
}

/// What the caller is choosing a session *for*.
///
/// The purpose travels instead of a pre-styled list because eligibility is a
/// presentation decision: a caller that filtered or greyed rows itself would
/// duplicate the rule at every call site, and they would drift. The selector
/// maps a purpose to a rule and the words that explain it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Choosing a session to share as a link.
    Share,
    /// Looking through sessions with no follow-on action.
    Browse,
}

/// Why a row cannot be chosen for the current purpose.
///
/// Only ever a display concern. The commands keep their own refusals — the
/// selector stops *offering* a choice they would reject, it never becomes the
/// thing that decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ineligible {
    pub reason: &'static str,
}

impl Purpose {
    /// Whether this row can be chosen, and if not, what to tell the user.
    pub fn eligibility(self, row: &SessionRow) -> Option<Ineligible> {
        match self {
            Purpose::Browse => None,
            Purpose::Share => match row.origin {
                Origin::Local => None,
                // Echoes the wording `share` itself uses when it refuses a
                // pulled session, so the greyed row and the error agree.
                Origin::Received => Some(Ineligible {
                    reason: "shared from another server",
                }),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(origin: Origin) -> SessionRow {
        SessionRow {
            id: "abc123".into(),
            title: "Refactor the auth guard".into(),
            agent: "claude".into(),
            turns: 12,
            started_at: DateTime::parse_from_rfc3339("2026-08-20T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            duration: Some(Duration::minutes(95)),
            project: Some("acme-app".into()),
            origin,
            prompted_by: Some("Ada".into()),
            context: Some("the login endpoint accepts an empty password".into()),
        }
    }

    #[test]
    fn sharing_refuses_a_session_that_came_from_another_server() {
        let ineligible = Purpose::Share.eligibility(&row(Origin::Received));
        assert_eq!(
            ineligible.map(|i| i.reason),
            Some("shared from another server")
        );
    }

    #[test]
    fn sharing_allows_a_locally_imported_session() {
        assert_eq!(Purpose::Share.eligibility(&row(Origin::Local)), None);
    }

    #[test]
    fn browsing_makes_every_origin_choosable() {
        assert_eq!(Purpose::Browse.eligibility(&row(Origin::Received)), None);
        assert_eq!(Purpose::Browse.eligibility(&row(Origin::Local)), None);
    }
}
