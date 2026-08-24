//! Turning a session into styled lines.
//!
//! Split from the interactive loop so a caller that only wants to print can use
//! it, and so the width behaviour is testable without a terminal.

use chrono::{DateTime, Duration, Utc};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::session::{Ineligible, Purpose, SessionRow};

/// Lines each session occupies in the list: a top border, the header, the
/// context line, and a bottom border. Fixed rather than derived so the scroll
/// arithmetic stays whole rows and a page never cuts a session in half.
pub const LINES_PER_ROW: usize = 4;

/// Blank columns kept between the list and each edge of the terminal, so the
/// rows read as a panel rather than text jammed against the frame.
pub const HORIZONTAL_MARGIN: u16 = 2;

/// Space between the left group and the right-aligned metadata.
const MIN_GAP: usize = 2;
/// Shortest title worth truncating to. Under this, the title is dropped rather
/// than rendered as a few characters and an ellipsis that identify nothing.
const MIN_TITLE_WIDTH: usize = 12;
/// The rail down the inside of every row's left edge, accented on the selected
/// one. Drawn on each content line so it spans the box's full height.
const CHIP: &str = " ▌ ";
/// Display columns the chip occupies. Counted in characters, not bytes — `█` is
/// three bytes and one column, and `len()` would over-reserve.
const CHIP_WIDTH: usize = 3;
/// Columns the box chrome takes from the row: the two border rules, the chip on
/// the left, and a space of padding on the right so text never touches the frame.
const BOX_CHROME: usize = 2 + CHIP_WIDTH + 1;

/// Theme hooks, so the caller's terminal decides the palette rather than this
/// crate hard-coding colours that may be invisible on a light background.
#[derive(Debug, Clone, Copy, Default)]
pub struct RowStyles {
    pub project: Style,
    pub title: Style,
    pub meta: Style,
    pub context: Style,
    /// Who is speaking, in a detail view. Brighter than the prose so a reader
    /// finds the turn boundaries before reading any of it.
    pub speaker: Style,
    pub accent: Style,
    /// The box drawn around an unselected row.
    pub border: Style,
    /// Text that is present but deliberately recessive — an unselected row's
    /// chip, an ineligible row's reason. Quieter than [`RowStyles::faint`],
    /// which is for prose that still has to be read.
    pub recessive: Style,
    /// Secondary prose: readable, but subordinate to [`RowStyles::context`].
    pub faint: Style,
    /// Applied to the part of a title or passage the query matched.
    pub hit: Style,
    pub selected: Style,
}

/// One session as [`LINES_PER_ROW`] lines, budgeted to `width`.
///
/// The header carries identity — project, title, and the right-aligned counts;
/// the second line carries context. An ineligible row is dimmed whole and
/// carries its reason where the author would be, which is the subtler signal: a
/// user scanning the list sees a quiet row, not a warning.
pub fn row_lines(
    row: &SessionRow,
    purpose: Purpose,
    width: usize,
    now: DateTime<Utc>,
    selected: bool,
    query: &str,
    styles: &RowStyles,
) -> Vec<Line<'static>> {
    let ineligible = purpose.eligibility(row);
    // The box is drawn as text rather than a Block so a row stays one unit the
    // caller can place, scroll, and test without a terminal.
    let inner = width.saturating_sub(BOX_CHROME);
    let edge = if selected {
        styles.accent
    } else {
        styles.border
    };
    let chip = if selected {
        styles.accent
    } else {
        styles.recessive
    };
    vec![
        box_edge(width, selected, true, edge),
        boxed(
            header_line(row, ineligible, inner, now, selected, query, styles),
            inner,
            edge,
            chip,
        ),
        boxed(
            context_line(row, ineligible, inner, query, styles),
            inner,
            edge,
            chip,
        ),
        box_edge(width, selected, false, edge),
    ]
}

/// Wrap arbitrary lines in the same box a row uses, so a header and a list row
/// read as the same object.
pub fn boxed_block(
    lines: Vec<Line<'static>>,
    width: usize,
    selected: bool,
    styles: &RowStyles,
) -> Vec<Line<'static>> {
    let edge = if selected {
        styles.accent
    } else {
        styles.border
    };
    let chip = if selected {
        styles.accent
    } else {
        styles.recessive
    };
    let inner = width.saturating_sub(BOX_CHROME);
    let mut out = vec![box_edge(width, selected, true, edge)];
    out.extend(lines.into_iter().map(|line| boxed(line, inner, edge, chip)));
    out.push(box_edge(width, selected, false, edge));
    out
}

/// The top or bottom rule of a row's box.
fn box_edge(width: usize, selected: bool, top: bool, edge: Style) -> Line<'static> {
    let (left, right) = if top { ("╭", "╮") } else { ("╰", "╯") };
    let span = width.saturating_sub(2);
    let rule = if selected { "━" } else { "─" };
    Line::from(Span::styled(
        format!("{left}{}{right}", rule.repeat(span)),
        edge,
    ))
}

/// Wrap a rendered line in the box's side borders, padded inside them.
///
/// The content is padded out to the full inner width first: a line shorter than
/// the box would otherwise put the closing border immediately after its text,
/// leaving a ragged right edge instead of a rectangle.
fn boxed(line: Line<'static>, inner: usize, edge: Style, chip: Style) -> Line<'static> {
    let drawn: usize = line
        .spans
        .iter()
        .map(|span| display_width(&span.content))
        .sum();
    let mut spans = vec![
        Span::styled("│", edge),
        // The chip runs the full height of the box, so every content line
        // carries it rather than only the header.
        Span::styled(CHIP, chip),
    ];
    spans.extend(line.spans);
    // Pad to the text budget, then the one trailing column the chrome reserves.
    spans.push(Span::raw(
        " ".repeat(inner.saturating_sub(drawn).saturating_add(1)),
    ));
    spans.push(Span::styled("│", edge));
    Line::from(spans)
}

fn header_line(
    row: &SessionRow,
    ineligible: Option<Ineligible>,
    width: usize,
    now: DateTime<Utc>,
    selected: bool,
    query: &str,
    styles: &RowStyles,
) -> Line<'static> {
    let dimmed = ineligible.is_some();
    let right = right_meta(row, now, width);
    let right_width = display_width(&right);
    let left_budget = width.saturating_sub(right_width).saturating_sub(MIN_GAP);

    let mut spans = Vec::new();
    let mut used = 0usize;

    // The project reads first and is never truncated away: it is how a reader
    // tells two similarly-titled sessions apart at a glance.
    if let Some(project) = row.project.as_deref() {
        let label = format!("{project} · ");
        if display_width(&label) + MIN_TITLE_WIDTH <= left_budget {
            used += display_width(&label);
            spans.push(Span::styled(
                label,
                if dimmed {
                    styles.recessive
                } else {
                    styles.project
                },
            ));
        }
    }

    let title_budget = left_budget.saturating_sub(used);
    let title = truncate(&row.title, title_budget);
    used += display_width(&title);
    let title_style = if dimmed {
        styles.recessive
    } else if selected {
        styles.title.add_modifier(Modifier::BOLD)
    } else {
        styles.title
    };
    spans.extend(highlighted(&title, query, title_style, styles.hit));

    if !right.is_empty() {
        let gap = width.saturating_sub(used + right_width).max(MIN_GAP);
        spans.push(Span::raw(" ".repeat(gap)));
        spans.push(Span::styled(
            right,
            if dimmed {
                styles.recessive
            } else {
                styles.meta
            },
        ));
    }
    Line::from(spans)
}

/// The second line: why this session is unavailable, or who ran it and what it
/// was about.
fn context_line(
    row: &SessionRow,
    ineligible: Option<Ineligible>,
    width: usize,
    query: &str,
    styles: &RowStyles,
) -> Line<'static> {
    if let Some(reason) = ineligible {
        return Line::from(Span::styled(
            truncate(reason.reason, width),
            styles.recessive,
        ));
    }

    let text = row.context.clone().unwrap_or_default();
    let text = match row.prompted_by.as_deref() {
        Some(who) if !text.is_empty() => format!("{who} · {text}"),
        Some(who) => who.to_string(),
        None => text,
    };
    Line::from(highlighted(
        &truncate(&text, width),
        query,
        styles.context,
        styles.hit,
    ))
}

/// The right-hand group: message count, duration, and how long ago it ran.
/// Parts are dropped cheapest-first until the row fits, so a narrow terminal
/// loses metadata rather than the title.
fn right_meta(row: &SessionRow, now: DateTime<Utc>, width: usize) -> String {
    let mut parts = vec![format!("{} msgs", row.turns)];
    if let Some(duration) = row.duration {
        parts.push(format_duration(duration));
    }
    parts.push(format_relative(row.started_at, now));

    while parts.len() > 1 {
        let candidate = parts.join(" · ");
        if display_width(&candidate) + MIN_TITLE_WIDTH + MIN_GAP <= width {
            return candidate;
        }
        parts.remove(0);
    }
    let last = parts.join(" · ");
    if display_width(&last) + MIN_TITLE_WIDTH + MIN_GAP <= width {
        return last;
    }
    String::new()
}

/// One session's detail, for a preview pane or for `show` to print.
pub fn detail_lines(
    row: &SessionRow,
    now: DateTime<Utc>,
    styles: &RowStyles,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            row.title.clone(),
            styles.title.add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(row.id.clone(), styles.faint)),
    ];
    let mut meta = vec![
        row.agent.clone(),
        format!("{} msgs", row.turns),
        format_relative(row.started_at, now),
    ];
    if let Some(duration) = row.duration {
        meta.insert(2, format_duration(duration));
    }
    if let Some(project) = row.project.as_deref() {
        meta.insert(0, project.to_string());
    }
    lines.push(Line::from(Span::styled(meta.join(" · "), styles.meta)));
    if let Some(who) = row.prompted_by.as_deref() {
        lines.push(Line::from(Span::styled(who.to_string(), styles.meta)));
    }
    lines
}

/// Wall-clock length, coarse on purpose: minutes distinguish two sessions,
/// seconds never do.
fn format_duration(duration: Duration) -> String {
    let minutes = duration.num_minutes().max(0);
    if minutes < 60 {
        return format!("{minutes}m");
    }
    format!("{}h {}m", minutes / 60, minutes % 60)
}

/// Relative for anything recent, because "yesterday" is easier to place than a
/// date; absolute once relative stops meaning anything.
fn format_relative(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let elapsed = now.signed_duration_since(at);
    let minutes = elapsed.num_minutes();
    if minutes < 1 {
        return "just now".to_string();
    }
    if minutes < 60 {
        return format!("{minutes} min ago");
    }
    let hours = elapsed.num_hours();
    if hours < 24 {
        return format!("{hours} hours ago");
    }
    let days = elapsed.num_days();
    if days == 1 {
        return "yesterday".to_string();
    }
    if days < 7 {
        return format!("{days} days ago");
    }
    at.format("%b %-d, %H:%M").to_string()
}

/// Split `text` into spans, styling every case-insensitive occurrence of a
/// query term with `hit` and the rest with `base`.
///
/// Matching here rather than asking the search for offsets: the query is what
/// the user typed and the passage is plain text, so the highlight is a property
/// of the two, not something a retriever has to report back per leg.
pub(crate) fn highlighted(text: &str, query: &str, base: Style, hit: Style) -> Vec<Span<'static>> {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .map(|term| term.to_lowercase())
        .collect();
    if terms.is_empty() {
        return vec![Span::styled(text.to_string(), base)];
    }

    let haystack = text.to_lowercase();
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = haystack.chars().collect();

    // Mark matched character positions, so overlapping terms merge into one
    // span instead of splitting each other.
    let mut marked = vec![false; chars.len()];
    for term in &terms {
        let needle: Vec<char> = term.chars().collect();
        if needle.is_empty() || needle.len() > lower.len() {
            continue;
        }
        for start in 0..=(lower.len() - needle.len()) {
            if lower[start..start + needle.len()] == needle[..] {
                marked[start..start + needle.len()].fill(true);
            }
        }
    }

    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_marked = marked.first().copied().unwrap_or(false);
    for (index, ch) in chars.iter().enumerate() {
        if marked[index] != run_marked {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                if run_marked { hit } else { base },
            ));
            run_marked = marked[index];
        }
        run.push(*ch);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if run_marked { hit } else { base }));
    }
    spans
}

/// Character count, not byte length: a multi-byte title must not be measured as
/// wider than it draws.
fn display_width(text: &str) -> usize {
    text.chars().count()
}

/// Truncate on a character boundary, marking the cut so a clipped title never
/// reads as the whole one.
fn truncate(text: &str, budget: usize) -> String {
    if budget == 0 {
        return String::new();
    }
    if display_width(text) <= budget {
        return text.to_string();
    }
    if budget <= 1 {
        return "…".to_string();
    }
    let kept: String = text.chars().take(budget - 1).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Origin;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 23, 12, 0, 0).unwrap()
    }

    fn row() -> SessionRow {
        SessionRow {
            id: "abc123".into(),
            title: "Refactor the auth guard so tenants resolve before the handler".into(),
            agent: "claude".into(),
            turns: 529,
            started_at: now() - Duration::hours(4),
            duration: Some(Duration::minutes(299)),
            project: Some("acme-app".into()),
            origin: Origin::Local,
            prompted_by: Some("Ada".into()),
            context: Some("the login endpoint accepts an empty password".into()),
        }
    }

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Index of the header line within a rendered row, past the box's top rule.
    const HEADER: usize = 1;
    /// Index of the context line within a rendered row.
    const CONTEXT: usize = 2;

    fn render(row: &SessionRow, purpose: Purpose, width: usize) -> Vec<String> {
        render_query(row, purpose, width, "")
    }

    fn render_query(row: &SessionRow, purpose: Purpose, width: usize, query: &str) -> Vec<String> {
        row_lines(
            row,
            purpose,
            width,
            now(),
            false,
            query,
            &RowStyles::default(),
        )
        .iter()
        .map(text)
        .collect()
    }

    #[test]
    fn a_row_is_a_header_and_a_context_line() {
        let lines = render(&row(), Purpose::Browse, 120);
        assert_eq!(lines.len(), LINES_PER_ROW);
        assert!(lines[HEADER].contains("acme-app"));
        assert!(lines[HEADER].contains("Refactor the auth guard"));
        assert!(lines[HEADER].contains("529 msgs"));
        assert!(lines[HEADER].contains("4h 59m"));
        assert!(lines[CONTEXT].contains("Ada"));
        assert!(lines[CONTEXT].contains("the login endpoint"));
    }

    #[test]
    fn no_line_draws_wider_than_its_budget() {
        for width in [20usize, 40, 60, 80, 100, 140] {
            for line in render(&row(), Purpose::Browse, width) {
                assert!(
                    line.chars().count() <= width,
                    "width {width} overflowed: {line:?}"
                );
            }
        }
    }

    #[test]
    fn a_short_context_line_still_closes_the_box_at_the_right_edge() {
        let mut brief = row();
        brief.context = Some("short".into());
        brief.prompted_by = None;
        let lines = render(&brief, Purpose::Browse, 80);
        // Every line of the box is the same width, so the right border forms a
        // straight edge instead of following the text.
        let widths: Vec<usize> = lines.iter().map(|line| line.chars().count()).collect();
        assert_eq!(widths, vec![80, 80, 80, 80], "box edges must align");
    }

    #[test]
    fn the_chip_runs_the_full_height_of_the_box() {
        let lines = render(&row(), Purpose::Browse, 80);
        let rail = CHIP.trim();
        assert!(lines[HEADER].contains(rail));
        assert!(lines[CONTEXT].contains(rail));
    }

    #[test]
    fn every_row_is_boxed_on_all_four_sides() {
        let lines = render(&row(), Purpose::Browse, 80);
        assert!(lines[0].starts_with('╭') && lines[0].ends_with('╮'));
        assert!(lines[HEADER].starts_with('│') && lines[HEADER].ends_with('│'));
        assert!(lines[CONTEXT].starts_with('│') && lines[CONTEXT].ends_with('│'));
        assert!(lines[3].starts_with('╰') && lines[3].ends_with('╯'));
    }

    #[test]
    fn a_selected_row_draws_a_heavier_box() {
        let plain = row_lines(
            &row(),
            Purpose::Browse,
            80,
            now(),
            false,
            "",
            &RowStyles::default(),
        );
        let picked = row_lines(
            &row(),
            Purpose::Browse,
            80,
            now(),
            true,
            "",
            &RowStyles::default(),
        );
        assert!(text(&plain[0]).contains('─'));
        assert!(text(&picked[0]).contains('━'));
    }

    #[test]
    fn a_narrow_row_keeps_the_title_and_sheds_metadata() {
        let lines = render(&row(), Purpose::Browse, 44);
        assert!(lines[HEADER].contains("Refactor"));
        assert!(!lines[HEADER].contains("529 msgs"));
    }

    #[test]
    fn an_ineligible_row_explains_itself_on_the_context_line() {
        let mut received = row();
        received.origin = Origin::Received;
        let lines = render(&received, Purpose::Share, 120);
        assert!(lines[CONTEXT].contains("shared from another server"));
        assert!(!lines[CONTEXT].contains("Ada"));
    }

    #[test]
    fn durations_read_in_hours_past_an_hour() {
        assert_eq!(format_duration(Duration::minutes(7)), "7m");
        assert_eq!(format_duration(Duration::minutes(59)), "59m");
        assert_eq!(format_duration(Duration::minutes(60)), "1h 0m");
        assert_eq!(format_duration(Duration::minutes(299)), "4h 59m");
    }

    #[test]
    fn recent_times_read_relatively_and_old_ones_by_date() {
        let now = now();
        assert_eq!(format_relative(now, now), "just now");
        assert_eq!(
            format_relative(now - Duration::minutes(19), now),
            "19 min ago"
        );
        assert_eq!(
            format_relative(now - Duration::hours(4), now),
            "4 hours ago"
        );
        assert_eq!(format_relative(now - Duration::days(1), now), "yesterday");
        assert_eq!(format_relative(now - Duration::days(3), now), "3 days ago");
        assert_eq!(
            format_relative(now - Duration::days(30), now),
            "Jul 24, 12:00"
        );
    }

    #[test]
    fn a_session_still_running_shows_no_duration() {
        let mut running = row();
        running.duration = None;
        let lines = render(&running, Purpose::Browse, 120);
        assert!(lines[HEADER].contains("529 msgs"));
        assert!(!lines[HEADER].contains("4h 59m"));
    }

    /// Styles whose `hit` is distinguishable from every other, so a test can
    /// tell a marked span from an unmarked one. A default `RowStyles` makes
    /// them all equal and every span looks marked.
    fn marking_styles() -> RowStyles {
        RowStyles {
            hit: Style::default().add_modifier(Modifier::BOLD),
            ..RowStyles::default()
        }
    }

    /// The matched runs of a rendered line, so a test asserts on what is
    /// highlighted rather than on span indices.
    fn hits(line: &Line<'_>) -> Vec<String> {
        line.spans
            .iter()
            .filter(|span| span.style.add_modifier.contains(Modifier::BOLD))
            .map(|span| span.content.to_string())
            .collect()
    }

    fn rendered_lines(row: &SessionRow, query: &str) -> Vec<Line<'static>> {
        row_lines(
            row,
            Purpose::Browse,
            120,
            now(),
            false,
            query,
            &marking_styles(),
        )
    }

    #[test]
    fn a_query_marks_where_it_matched() {
        let mut row = row();
        row.title = "Refactor the auth guard".into();
        row.context = Some("the auth guard rejects empty passwords".into());
        let lines = rendered_lines(&row, "auth");
        assert_eq!(hits(&lines[HEADER]), vec!["auth"]);
        assert_eq!(hits(&lines[CONTEXT]), vec!["auth"]);
    }

    #[test]
    fn matching_ignores_case() {
        let mut row = row();
        row.title = "Refactor the Auth guard".into();
        let lines = rendered_lines(&row, "auth");
        assert_eq!(hits(&lines[HEADER]), vec!["Auth"]);
    }

    #[test]
    fn every_term_of_a_multi_word_query_is_marked() {
        let mut row = row();
        row.title = "endpoint auth guard".into();
        let lines = rendered_lines(&row, "endpoint guard");
        assert_eq!(hits(&lines[HEADER]), vec!["endpoint", "guard"]);
    }

    #[test]
    fn an_empty_query_marks_nothing() {
        let lines = rendered_lines(&row(), "");
        assert!(hits(&lines[HEADER]).is_empty());
        assert!(hits(&lines[CONTEXT]).is_empty());
    }

    #[test]
    fn highlighting_never_drops_or_duplicates_text() {
        let text = "the auth guard rejects Auth tokens";
        let spans = highlighted(
            text,
            "auth",
            Style::default(),
            Style::default().add_modifier(Modifier::BOLD),
        );
        let rebuilt: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn overlapping_terms_merge_into_one_run() {
        let spans = highlighted(
            "authauth",
            "auth",
            Style::default(),
            Style::default().add_modifier(Modifier::BOLD),
        );
        let rebuilt: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(rebuilt, "authauth");
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn detail_names_the_session_and_its_id() {
        let lines = detail_lines(&row(), now(), &RowStyles::default());
        let rendered: Vec<String> = lines.iter().map(text).collect();
        assert!(rendered[0].contains("Refactor the auth guard"));
        assert!(rendered[1].contains("abc123"));
        assert!(rendered[2].contains("529 msgs"));
        assert!(rendered[2].contains("acme-app"));
    }
}
