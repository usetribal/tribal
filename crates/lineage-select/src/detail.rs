//! One session rendered for reading: a header, then the folded transcript.
//!
//! Exported apart from the interactive pane so a non-interactive caller can
//! print the same thing.

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::render::{boxed_block, highlighted, RowStyles};
use crate::session::SessionRow;
use crate::transcript::{activity_duration, activity_summary, activity_tools, Entry, Speaker};

/// Lines of a reply kept before it is cut short.
///
/// A reply runs to a median of 2,543 characters, roughly thirty wrapped lines.
/// Someone confirming a share needs to recognise the session, not re-read it, so
/// a long answer is trimmed with a marker saying what was left.
const MAX_REPLY_LINES: usize = 8;
/// Lines kept of anything else an entry says. Narration runs to a median of 99
/// characters, so this rarely bites.
const MAX_MESSAGE_LINES: usize = 4;
/// Tool names listed before a run reports a count instead.
const MAX_NAMED_TOOLS: usize = 3;
/// Columns kept clear on both sides of every entry, matching where the header
/// box's border sits so the pane has one continuous text column.
const GUTTER: usize = 2;
/// Columns the intermediate steps are pushed in by.
///
/// Replies and prompts sit at the left margin and narration sits inside it, so
/// the exchange a reader is scanning for reads as a spine with the machinery
/// hanging off it, rather than every entry claiming equal weight.
const INDENT_WIDTH: usize = 4;

/// The whole session as lines, wrapped to `width`.
/// The whole session as one run of lines, header included — for a caller that
/// prints rather than scrolls.
pub fn session_lines(
    row: &SessionRow,
    entries: &[Entry],
    width: usize,
    query: &str,
    styles: &RowStyles,
) -> Vec<Line<'static>> {
    let session = rendered_session(row, entries, width, query, styles);
    let mut lines = session.header;
    lines.extend(session.lines);
    lines
}

/// A rendered session, with the line each match landed on.
///
/// Positions come from the render rather than the raw text because a match is
/// only useful as somewhere to scroll to, and only the render knows which line
/// a word ended up on after wrapping.
/// One occurrence of the query in the rendered transcript.
///
/// A line can hold several, so a match is a span rather than a line: cycling
/// between lines would silently skip occurrences and leave the count disagreeing
/// with what a reader can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// Index into `lines`.
    pub line: usize,
    /// Index into that line's spans.
    pub span: usize,
}

pub struct RenderedSession {
    /// The header card, drawn above the scrolling body so which session is open
    /// stays on screen however far down the transcript a reader goes.
    pub header: Vec<Line<'static>>,
    /// The transcript, scrolled independently of the header.
    pub lines: Vec<Line<'static>>,
    /// Every occurrence, in reading order.
    pub matches: Vec<Match>,
}

pub fn rendered_session(
    row: &SessionRow,
    entries: &[Entry],
    width: usize,
    query: &str,
    styles: &RowStyles,
) -> RenderedSession {
    rendered_session_at(row, entries, width, query, None, styles)
}

/// As [`rendered_session`], with `current` naming the match the reader is on so
/// it can be picked out from the others.
pub fn rendered_session_at(
    row: &SessionRow,
    entries: &[Entry],
    width: usize,
    query: &str,
    current: Option<usize>,
    styles: &RowStyles,
) -> RenderedSession {
    let head = header(row, width, styles);
    let mut lines: Vec<Line<'static>> = Vec::new();
    if entries.is_empty() {
        lines.push(Line::default());
        lines.push(shift(
            Line::from(Span::styled(
                "This session has no turns to show.",
                styles.faint,
            )),
            GUTTER,
        ));
        return RenderedSession {
            header: head,
            lines,
            matches: Vec::new(),
        };
    }
    for entry in entries {
        // A rule before each prompt or answer, so the exchange reads as
        // discrete moments with the machinery gathered under each.
        if is_spine(entry) && !lines.is_empty() {
            lines.push(Line::default());
            lines.push(separator(width, styles));
        }
        lines.push(Line::default());
        lines.extend(entry_lines(entry, width, query, styles));
    }
    let matches = find_matches(&lines, query);
    if let Some(found) = current.and_then(|at| matches.get(at)).copied() {
        emphasise(&mut lines, found, styles);
    }
    RenderedSession {
        header: head,
        lines,
        matches,
    }
}

/// Pick the one occurrence the reader is on out of the others: underline that
/// span alone, and brighten the rules bounding the section it sits in so the eye
/// finds the right part of a long transcript.
fn emphasise(lines: &mut [Line<'static>], found: Match, styles: &RowStyles) {
    if let Some(span) = lines
        .get_mut(found.line)
        .and_then(|line| line.spans.get_mut(found.span))
    {
        span.style = span.style.add_modifier(Modifier::UNDERLINED);
    }
    for edge in section_bounds(lines, found.line) {
        if let Some(line) = lines.get_mut(edge) {
            for span in &mut line.spans {
                span.style = styles.accent;
            }
        }
    }
}

/// The separator rules immediately above and below `at`, which are what bound
/// the exchange the match sits in.
fn section_bounds(lines: &[Line<'static>], at: usize) -> Vec<usize> {
    let is_rule = |line: &Line<'static>| -> bool {
        let drawn: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let trimmed = drawn.trim();
        !trimmed.is_empty() && trimmed.chars().all(|c| c == '─')
    };
    let mut edges = Vec::new();
    if let Some(above) = (0..at).rev().find(|&index| is_rule(&lines[index])) {
        edges.push(above);
    }
    if let Some(below) = (at + 1..lines.len()).find(|&index| is_rule(&lines[index])) {
        edges.push(below);
    }
    edges
}

/// Every occurrence of the query, as the span it was drawn in.
///
/// The highlighter has already split each match into its own span, so an
/// occurrence is exactly one span whose text is one of the query's terms. Found
/// this way rather than by re-scanning the raw text, which would disagree with
/// what is on screen wherever wrapping or truncation intervened.
fn find_matches(lines: &[Line<'static>], query: &str) -> Vec<Match> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|term| !term.is_empty())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        for (span_index, span) in line.spans.iter().enumerate() {
            if terms.contains(&span.content.to_lowercase()) {
                found.push(Match {
                    line: line_index,
                    span: span_index,
                });
            }
        }
    }
    found
}

/// A faint rule closing off one entry.
///
/// Deliberately not the list's box: a transcript entry cannot be selected, and
/// giving it the same container would invite someone to try. A single rule marks
/// the boundary without implying the entry is an object to act on.
fn separator(width: usize, styles: &RowStyles) -> Line<'static> {
    Line::from(vec![
        Span::raw(" ".repeat(GUTTER)),
        Span::styled("─".repeat(width.saturating_sub(GUTTER * 2)), styles.border),
    ])
}

fn header(row: &SessionRow, width: usize, styles: &RowStyles) -> Vec<Line<'static>> {
    let mut facts = vec![row.agent.clone(), format!("{} msgs", row.turns)];
    if let Some(duration) = row.duration {
        let minutes = duration.num_minutes().max(0);
        facts.push(if minutes < 60 {
            format!("{minutes}m")
        } else {
            format!("{}h {}m", minutes / 60, minutes % 60)
        });
    }
    facts.push(row.started_at.format("%b %-d, %H:%M").to_string());
    if let Some(who) = row.prompted_by.as_deref() {
        facts.push(who.to_string());
    }

    // Laid out as the list row draws it — project, separator, title on one line,
    // details beneath — so opening a session moves nothing on screen. Selected
    // chrome, because the session is still the one picked in the list.
    let mut identity = Vec::new();
    if let Some(project) = row.project.as_deref() {
        identity.push(Span::styled(format!("{project} · "), styles.project));
    }
    identity.push(Span::styled(
        row.title.clone(),
        styles.title.add_modifier(Modifier::BOLD),
    ));
    boxed_block(
        vec![
            Line::from(identity),
            Line::from(Span::styled(facts.join(" · "), styles.meta)),
        ],
        width,
        true,
        styles,
    )
}

/// Whether an entry is part of the exchange or part of the machinery.
///
/// The prompts and the answers are what someone scanning a session is looking
/// for; narration and tool runs are how the agent got between them. Indenting
/// the second makes the first legible as a spine.
fn is_spine(entry: &Entry) -> bool {
    matches!(
        entry,
        Entry::Message {
            speaker: Speaker::User,
            ..
        } | Entry::Message { is_reply: true, .. }
    )
}

fn entry_lines(entry: &Entry, width: usize, query: &str, styles: &RowStyles) -> Vec<Line<'static>> {
    // Even an unindented entry keeps the gutter the box's border sits in, so the
    // left and right edges of the text hold all the way down the pane.
    let indent = GUTTER + if is_spine(entry) { 0 } else { INDENT_WIDTH };
    let body_width = width.saturating_sub(indent + GUTTER);
    let lines = match entry {
        Entry::Activity { turns } => {
            let mut parts = vec![activity_summary(turns)];
            if let Some(duration) = activity_duration(turns) {
                parts.push(duration);
            }
            let tools = activity_tools(turns);
            if !tools.is_empty() {
                parts.push(named_tools(&tools));
            }
            // One faint line, not an expandable strip: the session is on this
            // machine, and someone deciding whether to share it needs the shape
            // of the work rather than every call inside it.
            vec![Line::from(Span::styled(parts.join(" · "), styles.faint))]
        }
        Entry::Message {
            speaker,
            content,
            is_reply,
            tools,
        } => message_lines(
            *speaker, content, *is_reply, tools, body_width, query, styles,
        ),
    };
    lines.into_iter().map(|line| shift(line, indent)).collect()
}

/// Push a rendered line in from the margin.
fn shift(line: Line<'static>, columns: usize) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(columns))];
    spans.extend(line.spans);
    Line::from(spans)
}

fn message_lines(
    speaker: Speaker,
    content: &str,
    is_reply: bool,
    tools: &[String],
    width: usize,
    query: &str,
    styles: &RowStyles,
) -> Vec<Line<'static>> {
    // Every speaker label reads the same. The border and the indent already say
    // which entries are the exchange and which are machinery, so colouring the
    // labels differently as well says it twice and flattens the palette's real
    // job: separating who is speaking from what was said from what it did.
    let label = match speaker {
        Speaker::User => "you",
        Speaker::Agent => "agent",
    };

    let mut lines = vec![Line::from(Span::styled(
        label.to_string(),
        styles.speaker.add_modifier(Modifier::BOLD),
    ))];

    let budget = if is_reply || speaker == Speaker::User {
        MAX_REPLY_LINES
    } else {
        MAX_MESSAGE_LINES
    };
    let wrapped = wrap(content, width);
    let kept = wrapped.len().min(budget);
    for line in wrapped.iter().take(kept) {
        lines.push(Line::from(highlighted(
            line,
            query,
            styles.context,
            styles.hit,
        )));
    }
    if wrapped.len() > kept {
        lines.push(Line::from(Span::styled(
            format!("… {} more lines", wrapped.len() - kept),
            styles.faint,
        )));
    }
    if !tools.is_empty() {
        lines.push(Line::from(Span::styled(named_tools(tools), styles.faint)));
    }
    lines
}

/// Tool names up to a cap, past which the run reports a count — enough to show a
/// run's shape, beyond which the strip stops being scannable.
fn named_tools(tools: &[String]) -> String {
    if tools.len() <= MAX_NAMED_TOOLS {
        return tools.join(", ");
    }
    format!(
        "{}, +{} more",
        tools[..MAX_NAMED_TOOLS].join(", "),
        tools.len() - MAX_NAMED_TOOLS
    )
}

/// Break text to `width` on whitespace, keeping the author's own line breaks.
/// A word longer than the width is cut rather than allowed to overflow.
fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for paragraph in text.trim().lines() {
        if paragraph.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                word.chars().count()
            } else {
                line.chars().count() + 1 + word.chars().count()
            };
            if candidate <= width {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(word);
                continue;
            }
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            let mut long = word;
            while long.chars().count() > width {
                let head: String = long.chars().take(width).collect();
                out.push(head);
                let consumed: usize = long
                    .char_indices()
                    .nth(width)
                    .map(|(index, _)| index)
                    .unwrap_or(long.len());
                long = &long[consumed..];
            }
            line.push_str(long);
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Origin;
    use crate::transcript::TranscriptTurn;
    use chrono::{Duration, TimeZone, Utc};

    fn row() -> SessionRow {
        SessionRow {
            id: "abc123".into(),
            title: "Refactor the auth guard".into(),
            agent: "claude".into(),
            turns: 12,
            started_at: Utc.with_ymd_and_hms(2026, 8, 20, 10, 0, 0).unwrap(),
            duration: Some(Duration::minutes(95)),
            project: Some("acme-app".into()),
            origin: Origin::Local,
            prompted_by: Some("Ada".into()),
            context: None,
        }
    }

    fn text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_header_is_the_same_shape_as_the_list_row_it_came_from() {
        use crate::render::{row_lines, LINES_PER_ROW};
        let now = Utc.with_ymd_and_hms(2026, 8, 23, 12, 0, 0).unwrap();
        let listed = row_lines(
            &row(),
            crate::Purpose::Share,
            80,
            now,
            true,
            "",
            &RowStyles::default(),
        );
        let opened = session_lines(&row(), &[], 80, "", &RowStyles::default());

        // Opening a session must not move anything: same box height, same
        // widths, and the identity line reads identically.
        assert_eq!(opened[..LINES_PER_ROW].len(), listed.len());
        for (a, b) in listed.iter().zip(opened.iter()) {
            let width = |line: &Line<'_>| -> usize {
                line.spans.iter().map(|s| s.content.chars().count()).sum()
            };
            assert_eq!(width(a), width(b));
        }
        // The identity reads the same and starts at the same column; the row's
        // right-aligned counts move to the line below, which is the one
        // difference opening a session is allowed to make.
        assert!(text(&opened[1..2]).starts_with("│ ▌ acme-app · Refactor the auth guard"));
        assert!(text(&listed[1..2]).starts_with("│ ▌ acme-app · Refactor the auth guard"));
    }

    #[test]
    fn the_header_stays_highlighted_because_the_session_is_still_selected() {
        let lines = session_lines(&row(), &[], 80, "", &RowStyles::default());
        assert!(
            text(&lines[..1]).contains('━'),
            "the header keeps the selected rule"
        );
    }

    #[test]
    fn narration_is_readable_rather_than_faded_out() {
        let entries = vec![Entry::Message {
            speaker: Speaker::Agent,
            content: "Let me check the other consumer".into(),
            is_reply: false,
            tools: vec![],
        }];
        let styles = RowStyles {
            context: ratatui::style::Style::default().fg(ratatui::style::Color::Green),
            faint: ratatui::style::Style::default().fg(ratatui::style::Color::Red),
            ..RowStyles::default()
        };
        let lines = session_lines(&row(), &entries, 80, "", &styles);
        let body = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.contains("other consumer"))
            .expect("narration rendered");
        assert_eq!(body.style.fg, Some(ratatui::style::Color::Green));
    }

    #[test]
    fn the_header_names_the_session_and_its_facts() {
        let rendered = text(&session_lines(&row(), &[], 80, "", &RowStyles::default()));
        assert!(rendered.contains("Refactor the auth guard"));
        assert!(rendered.contains("acme-app"));
        assert!(rendered.contains("12 msgs"));
        assert!(rendered.contains("1h 35m"));
        assert!(rendered.contains("Ada"));
    }

    #[test]
    fn a_session_with_no_turns_says_so() {
        let rendered = text(&session_lines(&row(), &[], 80, "", &RowStyles::default()));
        assert!(rendered.contains("no turns to show"));
    }

    #[test]
    fn a_long_reply_is_cut_short_with_a_count() {
        let long = (0..40)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let entries = vec![Entry::Message {
            speaker: Speaker::Agent,
            content: long,
            is_reply: true,
            tools: vec![],
        }];
        let rendered = text(&session_lines(
            &row(),
            &entries,
            80,
            "",
            &RowStyles::default(),
        ));
        assert!(rendered.contains("… 32 more lines"), "{rendered}");
    }

    #[test]
    fn machinery_is_indented_and_the_exchange_is_not() {
        let entries = vec![
            Entry::Message {
                speaker: Speaker::User,
                content: "why?".into(),
                is_reply: false,
                tools: vec![],
            },
            Entry::Message {
                speaker: Speaker::Agent,
                content: "Let me look".into(),
                is_reply: false,
                tools: vec![],
            },
            Entry::Message {
                speaker: Speaker::Agent,
                content: "Because the salt is null.".into(),
                is_reply: true,
                tools: vec![],
            },
        ];
        let rendered = text(&session_lines(
            &row(),
            &entries,
            80,
            "",
            &RowStyles::default(),
        ));
        // Relative, not absolute: every entry sits inside the pane's gutter, and
        // what matters is that machinery is pushed in past the exchange.
        let indent_of = |label: &str| -> Vec<usize> {
            rendered
                .lines()
                .filter(|line| line.trim() == label)
                .map(|line| line.len() - line.trim_start().len())
                .collect()
        };
        assert_eq!(
            indent_of("you"),
            vec![GUTTER],
            "a prompt sits at the margin"
        );
        let agents = indent_of("agent");
        assert!(
            agents.contains(&(GUTTER + INDENT_WIDTH)),
            "narration is indented as machinery: {agents:?}"
        );
        assert!(
            agents.contains(&GUTTER),
            "the reply shares the prompt's margin: {agents:?}"
        );
    }

    #[test]
    fn searching_within_a_session_marks_the_matches() {
        let entries = vec![Entry::Message {
            speaker: Speaker::Agent,
            content: "the auth guard rejects empty passwords".into(),
            is_reply: true,
            tools: vec![],
        }];
        let styles = RowStyles {
            hit: ratatui::style::Style::default().add_modifier(Modifier::BOLD),
            ..RowStyles::default()
        };
        let lines = session_lines(&row(), &entries, 80, "auth", &styles);
        let marked: Vec<String> = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .filter(|span| span.style.add_modifier.contains(Modifier::BOLD))
            .map(|span| span.content.to_string())
            .collect();
        assert!(marked.contains(&"auth".to_string()), "{marked:?}");
    }

    #[test]
    fn matches_are_reported_by_the_line_they_landed_on() {
        let entries = vec![Entry::Message {
            speaker: Speaker::Agent,
            // Wraps at this width, so the two matches land on different lines
            // and the positions have to come from the render, not the source.
            content: format!("zebra {} zebra", "filler ".repeat(30)),
            is_reply: true,
            tools: vec![],
        }];
        let styles = RowStyles {
            hit: ratatui::style::Style::default().add_modifier(Modifier::BOLD),
            ..RowStyles::default()
        };
        // A term absent from the header, so the count is only the body's hits.
        let session = rendered_session(&row(), &entries, 60, "zebra", &styles);
        assert_eq!(session.matches.len(), 2, "{:?}", session.matches);
        assert!(
            session.matches[0].line < session.matches[1].line,
            "matches read in order"
        );
        assert!(session
            .matches
            .iter()
            .all(|found| found.line < session.lines.len()));
    }

    #[test]
    fn the_current_match_is_underlined_and_its_section_brightened() {
        let entries = vec![
            Entry::Message {
                speaker: Speaker::User,
                content: "first".into(),
                is_reply: false,
                tools: vec![],
            },
            Entry::Message {
                speaker: Speaker::User,
                content: "zebra here".into(),
                is_reply: false,
                tools: vec![],
            },
        ];
        let styles = RowStyles {
            accent: ratatui::style::Style::default().fg(ratatui::style::Color::Green),
            ..RowStyles::default()
        };
        let session = rendered_session_at(&row(), &entries, 80, "zebra", Some(0), &styles);

        let underlined: Vec<String> = session
            .lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .filter(|span| span.style.add_modifier.contains(Modifier::UNDERLINED))
            .map(|span| span.content.to_string())
            .collect();
        assert_eq!(underlined, vec!["zebra"]);

        // The rule bounding the match's section takes the accent, so the eye
        // finds the right part of a long transcript.
        let brightened = session.lines.iter().any(|line| {
            let drawn: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            drawn.trim().starts_with('─')
                && line
                    .spans
                    .iter()
                    .any(|s| s.style.fg == Some(ratatui::style::Color::Green))
        });
        assert!(brightened, "the section's rule should be accented");
    }

    #[test]
    fn nothing_is_emphasised_when_no_match_is_current() {
        let entries = vec![Entry::Message {
            speaker: Speaker::User,
            content: "zebra".into(),
            is_reply: false,
            tools: vec![],
        }];
        let session = rendered_session(&row(), &entries, 80, "zebra", &RowStyles::default());
        assert!(!session
            .lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .any(|span| span.style.add_modifier.contains(Modifier::UNDERLINED)));
    }

    #[test]
    fn the_header_is_separate_from_the_scrolling_body() {
        let entries = vec![Entry::Message {
            speaker: Speaker::User,
            content: "hello".into(),
            is_reply: false,
            tools: vec![],
        }];
        let session = rendered_session(&row(), &entries, 80, "", &RowStyles::default());
        // Kept apart so the caller can pin one and scroll the other; the body
        // must not repeat the title.
        assert!(text(&session.header).contains("Refactor the auth guard"));
        assert!(!text(&session.lines).contains("Refactor the auth guard"));
    }

    #[test]
    fn two_occurrences_on_one_line_are_two_matches() {
        let entries = vec![Entry::Message {
            speaker: Speaker::User,
            content: "zebra and zebra".into(),
            is_reply: false,
            tools: vec![],
        }];
        let session = rendered_session(&row(), &entries, 80, "zebra", &RowStyles::default());
        // Counting lines would call this one match and leave the count
        // disagreeing with what the reader can see.
        assert_eq!(session.matches.len(), 2);
        assert_eq!(session.matches[0].line, session.matches[1].line);
        assert!(session.matches[0].span < session.matches[1].span);
    }

    #[test]
    fn only_the_current_occurrence_is_underlined() {
        let entries = vec![Entry::Message {
            speaker: Speaker::User,
            content: "zebra and zebra".into(),
            is_reply: false,
            tools: vec![],
        }];
        let session = rendered_session_at(
            &row(),
            &entries,
            80,
            "zebra",
            Some(1),
            &RowStyles::default(),
        );
        let underlined: Vec<usize> = session
            .lines
            .iter()
            .flat_map(|line| line.spans.iter().enumerate())
            .filter(|(_, span)| span.style.add_modifier.contains(Modifier::UNDERLINED))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(underlined, vec![session.matches[1].span]);
    }

    #[test]
    fn a_session_with_no_query_reports_no_matches() {
        let entries = vec![Entry::Message {
            speaker: Speaker::Agent,
            content: "the auth guard".into(),
            is_reply: true,
            tools: vec![],
        }];
        let session = rendered_session(&row(), &entries, 80, "", &RowStyles::default());
        assert!(session.matches.is_empty());
    }

    #[test]
    fn an_activity_run_reads_as_one_line() {
        let turns = vec![TranscriptTurn {
            speaker: Speaker::Agent,
            content: String::new(),
            tools: vec!["Read".into(), "Edit".into()],
            wrote: vec!["src/auth.rs".into()],
            offset_seconds: None,
        }];
        let entries = vec![Entry::Activity { turns }];
        let rendered = text(&session_lines(
            &row(),
            &entries,
            80,
            "",
            &RowStyles::default(),
        ));
        assert!(rendered.contains("2 steps, wrote src/auth.rs"));
        assert!(rendered.contains("Read, Edit"));
    }

    #[test]
    fn no_rendered_line_exceeds_the_width() {
        let entries = vec![Entry::Message {
            speaker: Speaker::User,
            content: "a ".repeat(400),
            is_reply: false,
            tools: vec![],
        }];
        for line in session_lines(&row(), &entries, 60, "", &RowStyles::default()) {
            let drawn: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(drawn <= 60, "line overflowed: {drawn}");
        }
    }

    #[test]
    fn a_word_longer_than_the_width_is_broken_rather_than_overflowing() {
        let wrapped = wrap(&"x".repeat(25), 10);
        assert_eq!(wrapped.len(), 3);
        assert!(wrapped.iter().all(|line| line.chars().count() <= 10));
    }

    #[test]
    fn wrapping_keeps_the_authors_own_line_breaks() {
        assert_eq!(wrap("one\ntwo", 40), vec!["one", "two"]);
    }
}
