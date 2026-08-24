//! Drawing the share confirmation over the session.

use std::time::Instant;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::confirm::{Choice, Confirm, Stage};
use crate::orb;

/// The modal's interior, in cells. Wide enough that the orb reads as a sphere
/// rather than a blocky approximation — at much less than this the sampled
/// falloff has too few steps to show a curve.
const ORB_WIDTH: usize = 40;
const ORB_HEIGHT: usize = 17;
/// Chrome around the orb: border, title, the gap beneath it, and the answers.
const PADDING_X: u16 = 2;
const TITLE_ROWS: u16 = 2;
const ANSWER_ROWS: u16 = 2;

const TITLE: &str = "Share this session?";

/// Colours the modal uses, supplied by the caller like every other style in this
/// crate so the palette stays in one place.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModalStyles {
    pub border: Style,
    pub title: Style,
    /// The answer under the cursor.
    pub focused: Style,
    /// The answer that is not.
    pub unfocused: Style,
    /// The orb, and the rings it throws.
    pub orb: Style,
}

/// Draw the confirmation centred over whatever is already on screen.
pub fn draw(frame: &mut Frame, confirm: &Confirm, now: Instant, styles: &ModalStyles) {
    let area = centred(frame.area());
    // Punches a hole in what is behind, so the session shows around the modal
    // rather than through it.
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(styles.border),
        area,
    );

    let inner = Block::default().borders(Borders::ALL).inner(area);
    // Once the share is running there is nothing to ask and nothing to choose,
    // so the animation takes the whole interior. Splitting rows off for a title
    // and answers that are no longer shown would push the orb above the modal's
    // true centre and cut its waves short of the border.
    if confirm.stage() != Stage::Asking {
        draw_orb(frame, confirm, now, styles, inner);
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(TITLE_ROWS),
            Constraint::Min(1),
            Constraint::Length(ANSWER_ROWS),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(TITLE, styles.title)).centered()),
        rows[0],
    );
    draw_orb(frame, confirm, now, styles, rows[1]);
    draw_answers(frame, confirm, styles, rows[2]);
}

/// The modal's rectangle, centred in `area` and clamped so it still fits a
/// terminal smaller than it would like.
fn centred(area: Rect) -> Rect {
    let width = (ORB_WIDTH as u16 + PADDING_X * 2 + 2).min(area.width);
    let height = (ORB_HEIGHT as u16 + TITLE_ROWS + ANSWER_ROWS + 2).min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn draw_orb(frame: &mut Frame, confirm: &Confirm, now: Instant, styles: &ModalStyles, area: Rect) {
    // The field fills the space it is given rather than a fixed box, so the
    // waves reach the modal's own edges instead of stopping at an inner margin.
    //
    // Forced odd: an even field has no true centre cell, so the sampled rings
    // straddle two rows while the orb glyph sits on one, and the orb reads as
    // being off to one side of its own waves.
    let width = odd(area.width as usize);
    let height = odd(area.height as usize);
    if width == 0 || height == 0 {
        return;
    }

    let field = match confirm.stage() {
        Stage::Asking => orb::idle(width, height),
        _ => orb::frame(width, height, confirm.elapsed(now)),
    };

    let lines: Vec<Line> = (0..height)
        .map(|row| {
            let drawn: String = (0..width)
                .map(|column| field.glyph(column, row).unwrap_or(' '))
                .collect();
            Line::from(Span::styled(drawn, styles.orb)).centered()
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// The largest odd number that fits.
fn odd(n: usize) -> usize {
    if n % 2 == 0 {
        n.saturating_sub(1)
    } else {
        n
    }
}

fn draw_answers(frame: &mut Frame, confirm: &Confirm, styles: &ModalStyles, area: Rect) {
    // The answers disappear once the share is running: there is nothing left to
    // choose, and leaving them up would invite a second press.
    if confirm.stage() != Stage::Asking {
        return;
    }
    let spans = [Choice::GoBack, Choice::DoIt]
        .into_iter()
        .flat_map(|choice| {
            let focused = confirm.answer() == choice;
            let style = if focused {
                styles.focused.add_modifier(Modifier::BOLD)
            } else {
                styles.unfocused
            };
            let label = if focused {
                format!("[ {} ]", choice.label())
            } else {
                format!("  {}  ", choice.label())
            };
            [Span::styled(label, style), Span::raw("    ")]
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Line::from(spans).centered()),
        Rect {
            y: area.y + area.height.saturating_sub(1),
            height: 1,
            ..area
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::time::Duration;

    fn render(confirm: &Confirm, now: Instant, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw(frame, confirm, now, &ModalStyles::default()))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_modal_asks_and_offers_both_answers() {
        let drawn = render(&Confirm::new(), Instant::now(), 80, 30);
        assert!(drawn.contains(TITLE));
        assert!(drawn.contains("go back"));
        assert!(drawn.contains("do it"));
    }

    #[test]
    fn the_focused_answer_is_marked() {
        let drawn = render(&Confirm::new(), Instant::now(), 80, 30);
        assert!(drawn.contains("[ do it ]"), "sharing is the default");

        let mut confirm = Confirm::new();
        confirm.move_focus();
        assert!(render(&confirm, Instant::now(), 80, 30).contains("[ go back ]"));
    }

    #[test]
    fn a_running_share_takes_the_answers_away() {
        let mut confirm = Confirm::new();
        confirm.begin_work(Instant::now());
        let drawn = render(&confirm, Instant::now(), 80, 30);
        assert!(!drawn.contains("go back"), "nothing left to choose");
    }

    #[test]
    fn the_modal_fits_a_terminal_smaller_than_it_wants() {
        // Clamped rather than panicking or drawing outside the frame.
        let drawn = render(&Confirm::new(), Instant::now(), 30, 12);
        assert_eq!(drawn.lines().count(), 12);
        assert!(drawn.lines().all(|line| line.chars().count() == 30));
    }

    #[test]
    fn the_orb_rests_unfilled_and_fills_when_shared() {
        let resting = render(&Confirm::new(), Instant::now(), 80, 30);
        assert!(
            resting.contains(orb::ORB_EMPTY),
            "the unfilled orb should be visible at rest"
        );
        assert!(!resting.contains(orb::ORB_FULL), "but not yet filled");

        let mut confirm = Confirm::new();
        let start = Instant::now();
        confirm.begin_work(start);
        confirm.celebrate(start);
        let lit = render(&confirm, start + Duration::from_millis(600), 80, 30);
        assert!(
            lit.contains(orb::ORB_FULL),
            "a shared session fills the orb"
        );
    }
}
