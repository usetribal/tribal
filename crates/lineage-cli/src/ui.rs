//! Shared presentation for human-facing `tribal` stdout.
//!
//! Layout lives here so each command does not invent another `println!` shape.
//! Colour is clap's stack (`anstyle` / `anstream` detection via TTY + `NO_COLOR`)
//! and matches the inquire prompts: bright cyan accent, dim secondary. Formatters
//! that feed inquire (`list_rows`) stay unstyled — inquire paints its own list.
//!
//! This module is the only one allowed to `println!` (plus the init wizard's
//! box-drawing). Everywhere else, clippy `print_stdout` is deny.
#![allow(clippy::print_stdout)]

use std::fmt::Display;
use std::io::{self, IsTerminal};

use anstream::{AutoStream, ColorChoice};
use anstyle::{AnsiColor, Color, Style};
use chrono::{DateTime, Utc};
use lineage_core::{Confidence, Role};

const ACCENT: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightCyan)));
const ASSISTANT: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightGreen)));
const SYSTEM: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightYellow)));
const HIT_D: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightMagenta)));
const DIM: Style = Style::new().dimmed();
const BOLD: Style = Style::new().bold();
const HIT_RANK: [Style; 4] = [ACCENT, ASSISTANT, SYSTEM, HIT_D];

/// Widest title we keep before ellipsis, so a 26-char id still fits a 80-col
/// terminal beside turns / date / agent.
const TITLE_MAX: usize = 36;
const LABEL_WIDTH: usize = 9;

#[derive(Debug, Clone)]
pub struct ScanRow {
    pub title: String,
    pub id: String,
    pub turns: usize,
    pub day: String,
    pub agent: String,
    pub model: String,
    pub who: Option<String>,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct ScanWidths {
    title: usize,
    id: usize,
    turns: usize,
    agent: usize,
    model: usize,
    who: usize,
}

impl ScanWidths {
    fn measure(rows: &[ScanRow]) -> Self {
        let mut widths = Self {
            title: 0,
            id: 0,
            turns: 0,
            agent: 0,
            model: 0,
            who: 0,
        };
        for row in rows {
            widths.title = widths.title.max(display_width(&row.title).min(TITLE_MAX));
            widths.id = widths.id.max(display_width(&row.id));
            widths.turns = widths.turns.max(row.turns.to_string().len());
            widths.agent = widths.agent.max(display_width(&row.agent));
            widths.model = widths.model.max(display_width(&row.model));
            if let Some(who) = &row.who {
                widths.who = widths.who.max(display_width(who));
            }
        }
        widths
    }

    fn for_one(row: &ScanRow) -> Self {
        Self::measure(std::slice::from_ref(row))
    }
}

pub fn color_enabled() -> bool {
    match AutoStream::choice(&io::stdout()) {
        ColorChoice::Never => false,
        ColorChoice::Always | ColorChoice::AlwaysAnsi => true,
        ColorChoice::Auto => io::stdout().is_terminal(),
    }
}

const LOGO_PNG: &[u8] = include_bytes!("../assets/tribal-logo.png");
/// Columns for the mark after crop. Wide enough that the interlocking gaps
/// survive nearest-neighbour, not so wide that help falls off the screen.
const BANNER_WIDTH: u32 = 48;
const TITLE: &str = "tribal";
const TAGLINE: &str = "Provenance for every agent session";
const MARK: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::BrightWhite)))
    .bg_color(Some(Color::Ansi(AnsiColor::BrightWhite)));
const MARK_HALF: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightWhite)));

/// Tribal mark + nameplate above root `--help` and the interactive init wizard.
///
/// The PNG is thresholded and nearest-neighbour scaled, then printed as
/// half-blocks. The title sits in a collar the same width as the mark; the
/// tagline is inside it. Off a colour TTY (or under `NO_COLOR`) the mark is
/// skipped and the collar still prints.
pub fn banner() {
    if color_enabled() {
        if let Ok(img) = logo_image() {
            print_mark(&img);
        }
    }
    print_collar(TITLE, TAGLINE);
    println!();
}

fn logo_image() -> Result<image::RgbaImage, image::ImageError> {
    let img = image::load_from_memory(LOGO_PNG)?;
    Ok(crisp_mark(img.to_rgba8(), BANNER_WIDTH))
}

/// Binary white-or-nothing, cropped to the mark, nearest-neighbour to `cols`.
/// Grey anti-alias is what made the 24-column banner look out of focus.
fn crisp_mark(mut rgba: image::RgbaImage, cols: u32) -> image::RgbaImage {
    for pixel in rgba.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        let on = a > 128 && u16::from(r) + u16::from(g) + u16::from(b) > 380;
        pixel.0 = if on {
            [255, 255, 255, 255]
        } else {
            [0, 0, 0, 0]
        };
    }
    let cropped = crop_to_opaque(&rgba);
    let (cw, ch) = cropped.dimensions();
    if cw == 0 || ch == 0 {
        return rgba;
    }
    let mut pixel_h = ((ch as f64 / cw as f64) * f64::from(cols)).round() as u32;
    pixel_h = pixel_h.max(2);
    if pixel_h % 2 == 1 {
        pixel_h += 1;
    }
    image::imageops::resize(
        &cropped,
        cols,
        pixel_h,
        image::imageops::FilterType::Nearest,
    )
}

fn crop_to_opaque(rgba: &image::RgbaImage) -> image::RgbaImage {
    let mut min_x = rgba.width();
    let mut min_y = rgba.height();
    let mut max_x = 0;
    let mut max_y = 0;
    for (x, y, pixel) in rgba.enumerate_pixels() {
        if pixel.0[3] == 0 {
            continue;
        }
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    if min_x > max_x {
        return rgba.clone();
    }
    let pad = 1;
    let x = min_x.saturating_sub(pad);
    let y = min_y.saturating_sub(pad);
    let w = (max_x + 1 + pad).min(rgba.width()).saturating_sub(x);
    let h = (max_y + 1 + pad).min(rgba.height()).saturating_sub(y);
    image::imageops::crop_imm(rgba, x, y, w, h).to_image()
}

fn print_mark(img: &image::RgbaImage) {
    let width = img.width();
    let height = img.height();
    let mut y = 0;
    while y < height {
        print!("  ");
        for x in 0..width {
            let top = is_on(img.get_pixel(x, y));
            let bot = y + 1 < height && is_on(img.get_pixel(x, y + 1));
            match (top, bot) {
                (true, true) => print!("{}", paint_with(true, MARK, "█")),
                (false, true) => print!("{}", paint_with(true, MARK_HALF, "▄")),
                (true, false) => print!("{}", paint_with(true, MARK_HALF, "▀")),
                (false, false) => print!(" "),
            }
        }
        println!();
        y += 2;
    }
}

fn is_on(pixel: &image::Rgba<u8>) -> bool {
    pixel.0[3] > 0
}

fn print_collar(title: &str, tagline: &str) {
    let width = BANNER_WIDTH as usize;
    let rule = "─".repeat(width);
    println!("  {}{}{}", dim("╭"), dim(&rule), dim("╮"));
    println!(
        "  {}{}{}",
        dim("│"),
        paint(ACCENT.bold(), &pad_collar(&format!(" {title}"), width)),
        dim("│")
    );
    println!(
        "  {}{}{}",
        dim("│"),
        dim(pad_collar(&format!(" {tagline}"), width)),
        dim("│")
    );
    println!("  {}{}{}", dim("╰"), dim(&rule), dim("╯"));
}

fn pad_collar(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        return text.chars().take(width).collect();
    }
    format!("{text}{}", " ".repeat(width - len))
}

pub fn paint(style: Style, text: &str) -> String {
    paint_with(color_enabled(), style, text)
}

fn paint_with(enabled: bool, style: Style, text: &str) -> String {
    if enabled {
        format!("{style}{text}{style:#}")
    } else {
        text.to_string()
    }
}

pub fn accent(text: impl AsRef<str>) -> String {
    paint(ACCENT, text.as_ref())
}

pub fn dim(text: impl AsRef<str>) -> String {
    paint(DIM, text.as_ref())
}

pub fn ok(text: impl AsRef<str>) -> String {
    paint(ASSISTANT, text.as_ref())
}

pub fn caution(text: impl AsRef<str>) -> String {
    paint(SYSTEM, text.as_ref())
}

/// Colour-cycle a leading rank so hop / candidate lists scan like `context query`.
pub fn rank_label(rank: usize) -> String {
    let style = HIT_RANK[(rank.saturating_sub(1)) % HIT_RANK.len()];
    paint(style.bold(), &format!("{rank}."))
}

pub fn heading(title: &str) {
    println!("{}", paint(ACCENT.bold(), title));
}

pub fn section(title: &str) {
    println!("{}", paint(BOLD, title));
}

pub fn kv(label: &str, value: impl Display) {
    kv_width(label, value, LABEL_WIDTH);
}

pub fn kv_width(label: &str, value: impl Display, width: usize) {
    let label_col = format!("{label:<width$}");
    println!(
        "{}  {}",
        paint(DIM, &label_col),
        flag_paint(&value.to_string())
    );
}

pub fn empty(message: &str) {
    println!("{}", paint(DIM, message));
}

pub fn action(message: impl Display) {
    println!("{}", paint(BOLD, &message.to_string()));
}

pub fn indent(message: impl Display) {
    println!("  {message}");
}

/// Accent key, dim rest — import / pull ids, doctor links, upgrade steps.
pub fn row(key: impl Display, rest: impl Display) {
    indent(format!(
        "{}  {}",
        accent(key.to_string()),
        dim(rest.to_string())
    ));
}

pub fn warn(message: impl Display) {
    indent(format!("{} {}", paint(SYSTEM.bold(), "warning:"), message));
}

/// Ranked hit header for `context query` — the number cycles colour so 1 / 2 / 3
/// stay distinct when the summaries are long.
pub fn ranked_hit(rank: usize, strength: &str, attribution: &str) {
    let style = HIT_RANK[(rank.saturating_sub(1)) % HIT_RANK.len()];
    let number = paint(style.bold(), &format!("{rank}."));
    let badge = paint(DIM, &format!("[{strength}]"));
    indent(format!("{number} {badge} {attribution}"));
}

pub fn affordance(cmd: impl Display) {
    indent(format!("     {}", dim(format!("→ {cmd}"))));
}

pub fn hero(message: impl Display) {
    println!();
    println!("    {}", paint(ACCENT, &message.to_string()));
    println!();
}

pub fn blank() {
    println!();
}

/// Pretty JSON for `--json` / `--discover`. Machine-shaped; no colour.
pub fn json(value: &impl serde::Serialize) -> Result<(), serde_json::Error> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// One JSON value per line (`export --format jsonl`).
pub fn jsonl(value: &impl serde::Serialize) -> Result<(), serde_json::Error> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

/// Already-serialized machine text (hook JSON, `fork --brief`) with no extra newline.
pub fn raw(text: &str) {
    print!("{text}");
}

/// Already-serialized machine text plus a newline.
pub fn raw_line(text: impl Display) {
    println!("{text}");
}

/// RFC 3339 → `YYYY-MM-DD`. Time of day almost never distinguishes two sessions.
pub fn day(rfc3339: &str) -> &str {
    rfc3339.split('T').next().unwrap_or(rfc3339)
}

pub fn human_date(dt: DateTime<Utc>) -> String {
    dt.format("%-d %b %Y").to_string()
}

pub fn role_name(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Tool => "tool",
    }
}

fn role_style(role: Role) -> Style {
    match role {
        Role::User => ACCENT,
        Role::Assistant => ASSISTANT,
        Role::System => SYSTEM,
        Role::Tool => DIM,
    }
}

/// One turn in `show`: index, coloured role, optional model, preview.
pub fn turn(index: usize, role: Role, model: Option<&str>, preview: &str) {
    let role_col = paint(role_style(role), &format!("{:<9}", role_name(role)));
    let model_col = match model {
        Some(name) => paint(DIM, &format!(" ({name})")),
        None => String::new(),
    };
    let body = if preview.is_empty() {
        String::new()
    } else {
        format!("  {preview}")
    };
    indent(format!("{index:<3} {role_col}{model_col}{body}"));
}

pub fn confidence_name(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Exact => "exact",
        Confidence::Heuristic => "heuristic",
        Confidence::Manual => "manual",
    }
}

pub fn confidence_label(confidence: Confidence) -> String {
    let style = match confidence {
        Confidence::Exact => ASSISTANT,
        Confidence::Heuristic => SYSTEM,
        Confidence::Manual => ACCENT,
    };
    paint(style, confidence_name(confidence))
}

/// Paint standalone `true` / `false` / `yes` in kv values (doctor, show).
fn flag_paint(text: &str) -> String {
    if !color_enabled() {
        return text.to_string();
    }
    let mut out = text.to_string();
    for (token, style) in [("true", ASSISTANT), ("false", SYSTEM), ("yes", ASSISTANT)] {
        if out.contains(token) {
            out = out.replace(token, &paint_with(true, style, token));
        }
    }
    out
}

pub fn format_scan_rows(rows: &[ScanRow]) -> Vec<String> {
    format_scan_rows_with(rows, false)
}

pub fn print_scan_rows(rows: &[ScanRow]) {
    for line in format_scan_rows_with(rows, color_enabled()) {
        println!("{line}");
    }
}

pub fn format_scan_row(row: &ScanRow) -> String {
    format_one(row, &ScanWidths::for_one(row), false)
}

fn format_scan_rows_with(rows: &[ScanRow], color: bool) -> Vec<String> {
    let widths = ScanWidths::measure(rows);
    rows.iter()
        .map(|row| format_one(row, &widths, color))
        .collect()
}

fn format_one(row: &ScanRow, widths: &ScanWidths, color: bool) -> String {
    let title = pad_truncated(&row.title, widths.title);
    let title = if color {
        paint_with(true, ACCENT, &title)
    } else {
        title
    };
    let id = if color {
        paint_with(true, DIM, &pad(&row.id, widths.id))
    } else {
        pad(&row.id, widths.id)
    };
    let turns = format!("{:>width$} turns", row.turns, width = widths.turns);
    let mut line = format!(
        "{title}  {id}  {turns}  {}  {}  {}",
        pad(&row.day, 10),
        pad(&row.agent, widths.agent),
        pad(&row.model, widths.model)
    );
    if let Some(who) = &row.who {
        line.push_str("  ");
        line.push_str(&pad(who, widths.who));
    }
    if let Some(tag) = &row.tag {
        line.push_str("  ");
        line.push_str(tag);
    }
    line
}

fn pad(text: &str, width: usize) -> String {
    let len = display_width(text);
    if len >= width {
        return text.to_string();
    }
    format!("{text}{}", " ".repeat(width - len))
}

fn pad_truncated(text: &str, width: usize) -> String {
    let len = display_width(text);
    if len <= width {
        return pad(text, width);
    }
    let keep = width.saturating_sub(1);
    let mut out: String = text.chars().take(keep).collect();
    out.push('…');
    pad(&out, width)
}

fn display_width(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::Role;

    fn column(line: &str, needle: &str) -> usize {
        let bytes = line.find(needle).expect(line);
        line[..bytes].chars().count()
    }

    fn row(title: &str, id: &str) -> ScanRow {
        ScanRow {
            title: title.into(),
            id: id.into(),
            turns: 12,
            day: "2026-07-26".into(),
            agent: "claude".into(),
            model: "opus".into(),
            who: Some("Alice".into()),
            tag: None,
        }
    }

    #[test]
    fn mixed_titles_align_later_columns() {
        let rows = format_scan_rows(&[
            row("Lineage RLS audit", "01SHORT"),
            row("A much longer session title than the first", "01LONGID"),
        ]);
        let day_at: Vec<usize> = rows.iter().map(|line| column(line, "2026-07-26")).collect();
        assert_eq!(day_at[0], day_at[1], "{rows:?}");
        assert!(rows[1].contains('…'), "{}", rows[1]);
        assert!(!rows[0].contains('…'), "{}", rows[0]);
    }

    #[test]
    fn paint_is_plain_when_colour_is_off() {
        assert_eq!(paint_with(false, ACCENT, "title"), "title");
        assert!(paint_with(true, ACCENT, "title").contains("title"));
        assert_ne!(paint_with(true, ACCENT, "title"), "title");
    }

    #[test]
    fn role_and_confidence_are_lowercase_words() {
        assert_eq!(role_name(Role::User), "user");
        assert_eq!(role_name(Role::Assistant), "assistant");
        assert_eq!(confidence_name(Confidence::Exact), "exact");
    }

    #[test]
    fn each_role_has_its_own_colour() {
        let user = paint_with(true, role_style(Role::User), "user");
        let assistant = paint_with(true, role_style(Role::Assistant), "assistant");
        let tool = paint_with(true, role_style(Role::Tool), "tool");
        let system = paint_with(true, role_style(Role::System), "system");
        assert_ne!(user, assistant);
        assert_ne!(assistant, tool);
        assert_ne!(tool, system);
        assert_eq!(paint_with(false, role_style(Role::User), "user"), "user");
    }

    #[test]
    fn day_drops_the_clock() {
        assert_eq!(day("2026-07-26T09:31:04+00:00"), "2026-07-26");
    }

    #[test]
    fn each_confidence_has_its_own_colour() {
        let exact = paint_with(true, ASSISTANT, "exact");
        let heuristic = paint_with(true, SYSTEM, "heuristic");
        let manual = paint_with(true, ACCENT, "manual");
        assert_ne!(exact, heuristic);
        assert_ne!(heuristic, manual);
    }

    #[test]
    fn tagline_does_not_mention_git() {
        assert!(!TAGLINE.to_lowercase().contains("git"));
        assert_eq!(TAGLINE, "Provenance for every agent session");
    }

    #[test]
    fn embedded_logo_is_crisp_blocks() {
        let img = logo_image().expect("tribal mark");
        assert_eq!(img.width(), BANNER_WIDTH);
        assert_eq!(img.height() % 2, 0);
        let mut on = 0;
        let mut off = 0;
        for pixel in img.pixels() {
            match pixel.0 {
                [255, 255, 255, 255] => on += 1,
                [0, 0, 0, 0] => off += 1,
                other => panic!("logo must be binary, got {other:?}"),
            }
        }
        assert!(on > 0, "mark should be white");
        assert!(off > 0, "canvas should be empty");
    }
}
