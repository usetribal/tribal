//! Stripping Cursor's harness-injected XML-ish tag *markers* from turn text,
//! while keeping what they wrap.
//!
//! Cursor's IDE agent-transcript wraps nearly every user turn (confirmed:
//! 1208 of 1228 real user turns on a local machine) in `<user_query>...
//! </user_query>` plus a sibling `<timestamp>...</timestamp>`, and
//! occasionally injects blocks of tool-usage system prompt
//! (`<mcp_meta_tools>`, `<dynamic_tools>`), file-upload notices
//! (`<uploaded_documents>`, `<image_files>`), and web-search results
//! (`<external_links>`) around content still worth keeping — the search
//! results, the attached-file notice, the skill instructions are real
//! context for the turn, just not something a person typed themselves. Only
//! the tag markup is noise; the wrapped content stays. `<timestamp>` is the
//! one exception: its value is structured data extracted into
//! `Turn.timestamp`, not prose, so it comes out of the message body
//! entirely rather than being unwrapped in place.
//!
//! None of this wrapping is present in Claude Code transcripts, where a
//! plain user turn carries no tags at all — tags there only appear for
//! genuine harness events (`<command-name>` for a slash command,
//! `<local-command-stdout>` for its output).
//!
//! This only recognizes an exact, closed set of tag names — never a generic
//! `<[A-Za-z]+>` sweep — because real user messages routinely contain
//! angle-bracket text of their own (pasted TypeScript/JSX, terminal output
//! mentioning `<user_query>` as a literal string). Touching unknown tags
//! would corrupt that content; this module only ever acts on tags it can
//! name.

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};

/// Tags whose markers are stripped but whose content is kept — real context
/// for the turn (search results, attached-file notices, skill
/// instructions), just not text the person typed themselves.
const UNWRAP_TAGS: &[&str] = &[
    "user_query",
    "uploaded_documents",
    "external_links",
    "mcp_meta_tools",
    "mcp_meta_tool_servers",
    "dynamic_tools",
    "dynamic_tool_namespaces",
    "manually_attached_skills",
    "hooks_context",
    "image_files",
];

/// Result of stripping one turn's text: the message a person would
/// recognize as what they wrote (plus any kept context Cursor wrapped
/// alongside it), and the timestamp recovered from the wrapper — Cursor's
/// structured JSON carries no timestamp field anywhere, so this inline tag
/// is the only per-turn timing signal available for it.
pub struct StrippedCursorText {
    pub text: String,
    pub timestamp: Option<DateTime<Utc>>,
}

/// Strip Cursor's harness tag markers from a turn's raw text. `<timestamp>`
/// is removed along with its content (extracted into the returned
/// `timestamp` field instead — it's structured data, not prose). Every
/// other known tag (`<user_query>` and the noise-adjacent-but-worth-keeping
/// set) is unwrapped in place: markers gone, content kept. Text with none
/// of these tags (a `tool_result`, an already-plain assistant message)
/// passes through unchanged.
pub fn strip_cursor_tags(raw: &str) -> StrippedCursorText {
    let timestamp = extract_first_tag(raw, "timestamp").and_then(|s| parse_cursor_timestamp(&s));

    let mut text = raw.to_string();
    text = remove_tag_blocks(&text, "timestamp");
    for tag in UNWRAP_TAGS {
        text = unwrap_tag(&text, tag);
    }

    StrippedCursorText {
        text: text.trim().to_string(),
        timestamp,
    }
}

/// Removes every `<tag>...</tag>` block (tag *and* contents) from the input.
/// Only `<timestamp>` uses this — its value is structured data pulled out
/// into `Turn.timestamp` separately, so leaving the text behind in the
/// message body would duplicate it. Every other recognized tag is unwrapped
/// instead ([`unwrap_tag`]): the content is real context worth keeping, only
/// the markup is noise.
fn remove_tag_blocks(input: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        let Some(start) = rest.find(&open) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after_open = &rest[start + open.len()..];
        let Some(end) = after_open.find(&close) else {
            // Unclosed tag (truncated transcript) — drop from the open tag
            // to end of text rather than emit a dangling fragment.
            break;
        };
        rest = &after_open[end + close.len()..];
    }
    out
}

/// Replaces the **first** `<tag>...</tag>` pair with just its inner text,
/// leaving everything else in the string untouched — including the rest of
/// the tag name's literal text if it recurs.
///
/// This deliberately does not loop to unwrap every occurrence. Cursor writes
/// exactly one real `<user_query>` wrapper per turn; a repeat occurrence
/// seen in real data is pasted content (a user quoting another session's
/// transcript, or `tribal`'s own query output, back into a new chat) whose
/// open/close tags do not nest properly with the outer wrapper — naive
/// nearest-close pairing on repeated occurrences mismatches which close
/// belongs to which open and corrupts the paste. Only touching the first
/// pair sidesteps that ambiguity entirely and preserves the paste verbatim,
/// which is the same conservative choice [`remove_tag_blocks`] cannot make
/// (it discards *contents*, so a wrong pairing there would delete pasted
/// text — this only relocates text, so getting a later pair wrong is
/// harmless, but not attempting it at all is simpler and just as safe).
fn unwrap_tag(input: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(start) = input.find(&open) else {
        return input.to_string();
    };
    let after_open = &input[start + open.len()..];
    let Some(end) = after_open.find(&close) else {
        return input.to_string();
    };
    let mut out = String::with_capacity(input.len());
    out.push_str(&input[..start]);
    out.push_str(&after_open[..end]);
    out.push_str(&after_open[end + close.len()..]);
    out
}

fn extract_first_tag(input: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = input.find(&open)? + open.len();
    let end = input[start..].find(&close)? + start;
    Some(input[start..end].trim().to_string())
}

/// Parses Cursor's inline timestamp — `"Thursday, Jul 16, 2026, 8:03 PM
/// (UTC-7)"` — into a UTC instant. `chrono`'s format specifiers have
/// nothing for the trailing `(UTC±N)` suffix, so the offset is split off
/// and applied by hand rather than folded into one format string.
fn parse_cursor_timestamp(s: &str) -> Option<DateTime<Utc>> {
    let (body, offset_str) = s.rsplit_once('(')?;
    let offset_str = offset_str.strip_suffix(')')?.strip_prefix("UTC")?;
    let offset_hours: i32 = if offset_str.is_empty() {
        0
    } else {
        offset_str.parse().ok()?
    };
    let offset = FixedOffset::east_opt(offset_hours * 3600)?;

    let body = body.trim().trim_end_matches(',');
    // Drop the leading weekday name ("Thursday, ") — chrono can validate it
    // via %A, but the transcript's weekday is derived from the same instant
    // being parsed, not an independent field worth failing the parse over.
    let after_weekday = body.split_once(", ")?.1;
    let naive = NaiveDateTime::parse_from_str(after_weekday, "%b %e, %Y, %l:%M %p").ok()?;
    let local = offset.from_local_datetime(&naive).single()?;
    Some(local.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwraps_user_query_and_extracts_timestamp() {
        let raw = "<timestamp>Thursday, Jul 16, 2026, 8:03 PM (UTC-7)</timestamp>\n<user_query>\nuse pnpm not npm\n</user_query>";
        let result = strip_cursor_tags(raw);
        assert_eq!(result.text, "use pnpm not npm");
        let ts = result.timestamp.expect("timestamp parsed");
        assert_eq!(ts.to_rfc3339(), "2026-07-17T03:03:00+00:00");
    }

    #[test]
    fn parses_bare_utc_with_no_offset_digits() {
        let raw = "<timestamp>Saturday, Aug 22, 2026, 5:27 PM (UTC)</timestamp>\n<user_query>hi</user_query>";
        let result = strip_cursor_tags(raw);
        assert_eq!(
            result.timestamp.unwrap().to_rfc3339(),
            "2026-08-22T17:27:00+00:00"
        );
    }

    #[test]
    fn keeps_uploaded_documents_content_but_strips_its_tag_markers() {
        let raw = "<timestamp>Sunday, Aug 9, 2026, 2:56 PM (UTC+1)</timestamp>\n<uploaded_documents>\nThe following documents have been saved:\n- /tmp/foo.txt\n</uploaded_documents>\n<user_query>\nreview this file\n</user_query>";
        let result = strip_cursor_tags(raw);
        assert!(!result.text.contains("<uploaded_documents>"));
        assert!(!result.text.contains("</uploaded_documents>"));
        assert!(result
            .text
            .contains("The following documents have been saved"));
        assert!(result.text.contains("/tmp/foo.txt"));
        assert!(result.text.contains("review this file"));
    }

    #[test]
    fn keeps_mcp_meta_tools_content_but_strips_its_tag_markers() {
        let raw = "<mcp_meta_tools>\nYou have access to MCP tools through GetMcpTools...\n</mcp_meta_tools>\n<user_query>do the thing</user_query>";
        let result = strip_cursor_tags(raw);
        assert!(!result.text.contains("<mcp_meta_tools>"));
        assert!(result.text.contains("You have access to MCP tools"));
        assert!(result.text.contains("do the thing"));
    }

    #[test]
    fn plain_text_with_no_tags_passes_through_unchanged() {
        let raw = "hi, I am creating you briefly. Just say hi back";
        let result = strip_cursor_tags(raw);
        assert_eq!(result.text, raw);
        assert!(result.timestamp.is_none());
    }

    /// The exact hazard this module exists to avoid: a real Cursor turn
    /// pasted terminal output that contained the literal string
    /// `user_query` inside pasted content, not as a wrapper tag. A stripper
    /// that recognized bare word occurrences (rather than the `<tag>...
    /// </tag>` structural pair) would eat legitimate pasted content.
    #[test]
    fn does_not_corrupt_pasted_content_that_mentions_tag_names_as_plain_words() {
        let raw = "<timestamp>Sunday, Aug 23, 2026, 6:56 PM (UTC+1)</timestamp>\n<user_query>\ncan we use colour to differentiate turns, e.g. user_query blocks in the logs\n</user_query>";
        let result = strip_cursor_tags(raw);
        assert_eq!(
            result.text,
            "can we use colour to differentiate turns, e.g. user_query blocks in the logs"
        );
    }

    /// Pasted TypeScript/JSX containing angle-bracket syntax must survive
    /// untouched — this module only recognizes an exact, closed tag-name
    /// list, never a generic `<[A-Za-z]+>` sweep.
    #[test]
    fn pasted_code_with_angle_brackets_is_not_treated_as_a_tag() {
        let raw = "<user_query>\nfix this type: type Foo = AuthedRequest<AuthSessionResponse>;\n</user_query>";
        let result = strip_cursor_tags(raw);
        assert_eq!(
            result.text,
            "fix this type: type Foo = AuthedRequest<AuthSessionResponse>;"
        );
    }

    #[test]
    fn unclosed_tag_from_a_truncated_transcript_does_not_panic() {
        let raw = "<user_query>\nunterminated";
        let result = strip_cursor_tags(raw);
        // No panic is the assertion; content recovery from a truncated
        // transcript is best-effort, not a contract.
        let _ = result.text;
    }
}

#[cfg(test)]
mod real_data_repro {
    use super::*;

    /// A real turn (content transcribed, paths and identifiers changed)
    /// pasted `tribal`'s own query output — which itself echoes other
    /// sessions' raw `<timestamp>`/`<user_query>` tags — back into a Cursor
    /// chat. That paste's tag pairs do not nest cleanly with the turn's own
    /// outer wrapper (its close lands after two more opens), which broke an
    /// earlier version of `unwrap_tag` that looped to unwrap every
    /// occurrence: it mismatched which close belonged to which open and
    /// left a stray `<user_query>` in the output. Only ever unwrapping the
    /// first pair sidesteps the mismatch and keeps the paste intact.
    const REAL_TURN_WITH_PASTED_QUERY_OUTPUT: &str = "<timestamp>Sunday, Aug 23, 2026, 6:56 PM (UTC+1)</timestamp>\n<user_query>\ncan we use colour to differentiate turns\n\n1. [medium] cursor session abc123, 2026-08-23\n       <timestamp>Sunday, Aug 23, 2026, 6:02 PM (UTC+1)</timestamp>\n       <user_query>\n       nested pasted query one\n       </user_query>\n  2. [medium] cursor session abc123, 2026-08-23\n       <timestamp>Sunday, Aug 23, 2026, 6:10 PM (UTC+1)</timestamp>\n       <user_query>\n       nested pasted query two\n       </user_query>\n</user_query>";

    #[test]
    fn only_unwraps_the_outer_wrapper_leaving_a_non_nesting_paste_intact() {
        let result = strip_cursor_tags(REAL_TURN_WITH_PASTED_QUERY_OUTPUT);
        // The outer wrapper is gone from the start of the message...
        assert!(result
            .text
            .starts_with("can we use colour to differentiate turns"));
        // ...but the pasted tags inside are preserved verbatim rather than
        // partially stripped or corrupted.
        assert!(result
            .text
            .contains("<user_query>\n       nested pasted query one"));
        assert!(result
            .text
            .contains("<user_query>\n       nested pasted query two"));
    }
}
