use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use crate::ids::LineageId;
use crate::salience::turn_is_salient;
use crate::{ArtifactKind, Conversation, Role, Turn};

/// Claude Code's own session title from a `"type":"summary"` transcript entry.
pub const SESSION_SUMMARY_KEY: &str = "session_summary";

/// Modification time of the transcript at the moment it was last read, RFC 3339.
///
/// The incremental import skips a session whose transcript has not been written
/// since this stamp. It cannot use `ended_at` for that: `ended_at` is the last
/// *turn's* timestamp, while the vendor keeps writing records after it, so the
/// file's mtime is reliably later than `ended_at` on every session and nothing
/// would ever skip.
pub const SOURCE_MTIME_KEY: &str = "source_mtime";
/// Heuristic summary generated at import when no vendor summary exists.
pub const ARCHITECTURE_SUMMARY_KEY: &str = "architecture_summary";

const DEFAULT_OPENING_ASK_CHARS: usize = 160;
const ID_PREFIX_CHARS: usize = 8;

const CODE_EDIT_TOOLS: &[&str] = &[
    "edit",
    "edit_file",
    "write",
    "write_file",
    "str_replace",
    "apply_patch",
    "search_replace",
    "multiedit",
    "create",
    "patch",
];

/// True when the conversation contains evidence of workspace file modifications.
pub fn conversation_modified_code(conv: &Conversation) -> bool {
    for turn in &conv.turns {
        if turn_modified_code(turn) {
            return true;
        }
    }
    false
}

pub fn turn_modified_code(turn: &Turn) -> bool {
    for artifact in &turn.artifacts {
        if matches!(artifact.kind, ArtifactKind::FileEdit | ArtifactKind::Diff) {
            return true;
        }
    }
    for tc in &turn.tool_calls {
        let name = tc.name.to_lowercase();
        if CODE_EDIT_TOOLS.iter().any(|t| name.contains(t)) {
            return true;
        }
    }
    false
}

/// Paths the conversation *wrote* (edit/diff artifacts only) — the authorship
/// signal, deliberately excluding tool-call reads so consumers like link
/// gating are not polluted by files the session merely consulted.
pub fn files_written(conv: &Conversation) -> Vec<String> {
    let mut paths: Vec<String> = conv
        .turns
        .iter()
        .flat_map(|t| &t.artifacts)
        .filter(|a| {
            matches!(a.kind, ArtifactKind::FileEdit | ArtifactKind::Diff) && !a.path.is_empty()
        })
        .map(|a| a.path.clone())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Repo-relative or logical paths touched by code-changing artifacts and tools.
pub fn files_touched(conv: &Conversation) -> Vec<String> {
    let mut paths = Vec::new();
    for turn in &conv.turns {
        for artifact in &turn.artifacts {
            if matches!(
                artifact.kind,
                ArtifactKind::FileEdit | ArtifactKind::Diff | ArtifactKind::TerminalCommand
            ) && !artifact.path.is_empty()
                && !artifact.path.starts_with("turn-")
            {
                paths.push(artifact.path.clone());
            }
        }
        for tc in &turn.tool_calls {
            if let Some(path) = extract_path_from_tool_args(&tc.arguments) {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

fn extract_path_from_tool_args(args: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(args) {
        for key in ["path", "file_path", "file", "target_file"] {
            if let Some(p) = v.get(key).and_then(|x| x.as_str()) {
                if !p.is_empty() {
                    return Some(p.to_string());
                }
            }
        }
    }
    None
}

/// Per-snippet cap for edit text in the retrieval body. An edit's `new_string`
/// can be an entire file (whole-file writes, large diffs); embedding or indexing
/// megabytes of code per turn is both memory-ruinous for the dense model and
/// low-signal (the intent lives in the first lines — signatures, identifiers —
/// not the 2000th line of a generated file). Truncating to a prefix keeps the
/// matchable signal without the blowup.
const MAX_SNIPPET_CHARS: usize = 800;

/// Take at most `max` chars, on a char boundary. Used to bound edit snippets so
/// one giant write cannot dominate a turn's retrieval text.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

/// Retrieval text for one turn: prose plus the identifiers a search query is
/// likely to name — tool-call names, the file paths they touched, and edit
/// snippets. Prose-only indexing (the previous `indexable_body`) missed
/// identifiers like `rebuild-index` that appear only in a tool call or path, so
/// a lexical query for them found nothing even when the session was about
/// exactly that. This is the shared corpus unit: the whole-session FTS body
/// (`enriched_indexable_body`) joins it across turns, and dense chunking groups
/// it (see `session_chunks`). Edit snippets are capped (`MAX_SNIPPET_CHARS`) so a
/// whole-file write does not bloat the index or blow up the embedder.
pub fn turn_indexable_text(turn: &Turn) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !turn.content.trim().is_empty() {
        parts.push(turn.content.clone());
    }
    for tc in &turn.tool_calls {
        if !tc.name.is_empty() {
            parts.push(tc.name.clone());
        }
        if let Some(path) = extract_path_from_tool_args(&tc.arguments) {
            parts.push(path);
        }
    }
    for artifact in &turn.artifacts {
        if !artifact.path.is_empty() && !artifact.path.starts_with("turn-") {
            parts.push(artifact.path.clone());
        }
        // Edit snippets carry the actual changed code; `new_string` is the
        // post-image that exists in the committed file, `old_string` the text
        // the edit replaced — both are high-signal for "what did this session
        // do to this code". Capped to a prefix so a whole-file write cannot
        // dominate (see `MAX_SNIPPET_CHARS`).
        if let Some(resolve) = &artifact.resolve {
            if let Some(new_string) = &resolve.new_string {
                parts.push(truncate_chars(new_string, MAX_SNIPPET_CHARS));
            }
            if let Some(old_string) = &resolve.old_string {
                parts.push(truncate_chars(old_string, MAX_SNIPPET_CHARS));
            }
        }
    }
    parts.join("\n")
}

/// The whole-session retrieval body: every turn's enriched text, joined. This
/// is the FTS document — BM25 handles long documents, so the session is indexed
/// whole (dense retrieval chunks instead; see `session_chunks`).
pub fn enriched_indexable_body(conv: &Conversation) -> String {
    conv.turns
        .iter()
        .map(turn_indexable_text)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Default chunk size for dense embedding, in characters. An intent-bearing
/// turn-group is the natural unit; this only bounds a runaway group (a long
/// tool-heavy stretch) so one chunk's embedding does not average away its
/// signal. Chosen for the SE domain (roughly a few hundred tokens); a tunable,
/// not a fixed law — the eval stage calibrates it (gotcha R2.3).
pub const DEFAULT_CHUNK_MAX_CHARS: usize = 2000;

/// One dense-retrieval chunk: enriched text plus the turn dense evidence
/// should point at when this chunk matches. The anchor is the group's first
/// user turn if one contributed text, else its first salient contributor — the
/// turn whose words carried the intent, not merely the first turn in the group.
/// (Under binary salience every contributor is equally salient, so "first user
/// turn, else first contributor" is what "most salient contributor" now means.)
#[derive(Debug, Clone)]
pub struct SessionChunk {
    pub anchor_turn_id: LineageId,
    pub text: String,
}

/// Dense-retrieval chunks: the session split into intent-bearing turn-groups,
/// each a user turn plus the assistant turns that answer it, as enriched text.
/// Chunking (not whole-session embedding) keeps a short query from being
/// averaged against a whole session's vector (gotcha R2.3). Groups prefer turn
/// boundaries, but **no chunk ever exceeds `max_chars`**: a single turn whose
/// text is larger than the budget is split into sub-chunks, so a giant edit
/// cannot produce a multi-megabyte chunk that starves the embedder. Zero-weight
/// turns (tool results, pure exploration) contribute no text, so a group of
/// only noise produces no chunk at all.
pub fn session_chunks(conv: &Conversation, max_chars: usize) -> Vec<SessionChunk> {
    let max_chars = max_chars.max(1);
    let mut chunks = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_len = 0usize;
    // (is_user, turn id) of the group's anchor: the first user turn that
    // contributed text, else the first salient contributor. A non-user anchor
    // is only ever upgraded to a user turn, never to a later contributor, so a
    // group anchored by its user turn stays there.
    let mut anchor: Option<(bool, LineageId)> = None;

    let flush = |current: &mut Vec<String>,
                 current_len: &mut usize,
                 anchor: &mut Option<(bool, LineageId)>,
                 chunks: &mut Vec<SessionChunk>| {
        if let Some((_, anchor_turn_id)) = anchor.take() {
            if !current.is_empty() {
                chunks.push(SessionChunk {
                    anchor_turn_id,
                    text: current.join("\n"),
                });
            }
        }
        current.clear();
        *current_len = 0;
    };

    for turn in &conv.turns {
        // A user turn starts a new intent group — it is the question the
        // following assistant turns answer, and the unit a query should match.
        if turn.role == Role::User {
            flush(&mut current, &mut current_len, &mut anchor, &mut chunks);
        }

        if !turn_is_salient(turn) {
            continue;
        }
        let text = turn_indexable_text(turn);
        if text.is_empty() {
            continue;
        }
        let is_user = turn.role == Role::User;
        // Set the anchor on the first contributor; only a user turn may replace
        // a non-user anchor (the "first user turn, else first contributor" rule).
        if anchor.is_none() || (is_user && !anchor.as_ref().unwrap().0) {
            anchor = Some((is_user, turn.id.clone()));
        }

        for piece in split_to_max(&text, max_chars) {
            // Close the current group before a piece that would overflow it, so
            // no emitted chunk exceeds the budget. The overflow chunk keeps the
            // group's anchor: it is still the same intent, just more of it.
            if current_len + piece.len() > max_chars {
                let carried = anchor.clone();
                flush(&mut current, &mut current_len, &mut anchor, &mut chunks);
                anchor = carried;
            }
            current_len += piece.len();
            current.push(piece);
        }
    }
    flush(&mut current, &mut current_len, &mut anchor, &mut chunks);
    chunks
}

/// Split text into pieces of at most `max_chars` characters, on char
/// boundaries. A turn whose enriched text exceeds a whole chunk budget (a large
/// edit or tool dump) is broken up here so no single chunk is unbounded.
fn split_to_max(text: &str, max_chars: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return vec![text.to_string()];
    }
    chars
        .chunks(max_chars)
        .map(|c| c.iter().collect())
        .collect()
}

/// Human-readable label for a session in lists, pickers, and the web UI.
///
/// Precedence: vendor summary → architecture summary (first line) → opening ask
/// → id prefix. Each candidate is stripped of harness markup (slash-command
/// XML, `claude (…)` wrappers, timestamps) so a list is a list of topics, not
/// of `<command-name>/model</command-name>`. Callers that need the raw id keep
/// using `conv.id`.
pub fn display_title(conv: &Conversation) -> String {
    if let Some(title) = metadata_line(conv, SESSION_SUMMARY_KEY) {
        if let Some(clean) = usable_title(&title) {
            return clean;
        }
    }
    if let Some(summary) = metadata_line(conv, ARCHITECTURE_SUMMARY_KEY) {
        let first = summary.lines().next().unwrap_or(summary.as_str());
        if let Some(clean) = usable_title(first) {
            return clean;
        }
    }
    if let Some(ask) = opening_ask(conv, DEFAULT_OPENING_ASK_CHARS) {
        return ask;
    }
    id_prefix(conv.id.as_str())
}

/// The first non-empty user turn, flattened and truncated — the session's own
/// data, not an LLM summary.
pub fn opening_ask(conv: &Conversation, max_chars: usize) -> Option<String> {
    let first = conv
        .turns
        .iter()
        .find(|turn| turn.role == Role::User && !turn.content.trim().is_empty())?;
    let flat = first
        .content
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let clean = usable_title(&flat)?;
    Some(truncate_line(&clean, max_chars))
}

/// Short lineage id for secondary labels (`019fa49d…`).
pub fn id_prefix(id: &str) -> String {
    if id.chars().count() <= ID_PREFIX_CHARS {
        return id.to_string();
    }
    format!("{}…", id.chars().take(ID_PREFIX_CHARS).collect::<String>())
}

/// Flatten harness chrome for a person: strip XML tags, unwrap `claude (…)`
/// wrappers. A title that is only a slash command or a timestamp becomes
/// empty so the caller can fall back.
pub fn humanize_text(raw: &str) -> String {
    usable_title(raw).unwrap_or_default()
}

/// Claude and Cursor stamp slash commands and chrome into the vendor summary.
/// Those are facts about the harness, not a title a person can scan.
fn usable_title(raw: &str) -> Option<String> {
    let stripped = strip_markup(raw);
    let unwrapped = unwrap_agent_wrapper(&stripped);
    let collapsed = unwrapped.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() || is_noise_title(&collapsed) {
        return None;
    }
    Some(collapsed)
}

fn strip_markup(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '<' {
            for next in chars.by_ref() {
                if next == '>' {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn unwrap_agent_wrapper(raw: &str) -> &str {
    let Some(open) = raw.find(" (") else {
        return raw;
    };
    if !raw.ends_with(')') {
        return raw;
    }
    let prefix = &raw[..open];
    if !is_agent_prefix(prefix) {
        return raw;
    }
    &raw[open + 2..raw.len() - 1]
}

fn is_agent_prefix(prefix: &str) -> bool {
    matches!(
        prefix,
        "claude" | "cursor" | "codex" | "copilot" | "gemini" | "chatgpt"
    )
}

fn is_noise_title(title: &str) -> bool {
    if title.starts_with('/') {
        return true;
    }
    if title.eq_ignore_ascii_case("[image]") || title.eq_ignore_ascii_case("image") {
        return true;
    }
    if title.eq_ignore_ascii_case("user_query") || title.eq_ignore_ascii_case("user query") {
        return true;
    }
    if looks_like_timestamp(title) {
        return true;
    }
    title.chars().all(|c| !c.is_alphanumeric())
}

fn looks_like_timestamp(title: &str) -> bool {
    let has_clock = title.contains("UTC")
        || title.contains("GMT")
        || title.contains(" AM")
        || title.contains(" PM");
    has_clock && title.chars().any(|c| c.is_ascii_digit())
}

fn metadata_line(conv: &Conversation, key: &str) -> Option<String> {
    conv.metadata
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Heuristic architecture/decision summary from session content (no LLM).
pub fn generate_architecture_summary(conv: &Conversation) -> String {
    let title = conv
        .turns
        .iter()
        .find(|t| t.role == Role::User)
        .map(|t| truncate_line(&t.content, 200))
        .unwrap_or_else(|| "Agent session".into());

    let files = files_touched(conv);
    let file_line = if files.is_empty() {
        "Files: (none detected)".to_string()
    } else if files.len() <= 5 {
        format!("Files: {}", files.join(", "))
    } else {
        format!(
            "Files: {} (+{} more)",
            files[..3].join(", "),
            files.len() - 3
        )
    };

    let model = conv
        .primary_model()
        .map(|m| format!("Model: {m}"))
        .unwrap_or_default();

    let mut parts = vec![format!("{} ({})", conv.agent.as_str(), title), file_line];
    if !model.is_empty() {
        parts.push(model);
    }
    parts.join("\n")
}

fn truncate_line(s: &str, max: usize) -> String {
    let one_line: String = s.lines().next().unwrap_or(s).trim().to_string();
    if one_line.chars().count() <= max {
        one_line
    } else {
        format!("{}…", one_line.chars().take(max).collect::<String>())
    }
}

/// Monotonic merge of a conversation's `ended_at` across two copies of the same
/// session.
///
/// Absence means "no end recorded", not "ended at the beginning of time", so a
/// copy that knows an end time is never regressed to unknown by one that does
/// not. Order-independent, which is what lets a push and a pull of the same
/// session converge to identical state whichever runs first
/// (`docs/sync-semantics.md`, property 3).
pub fn merge_ended_at(
    local: Option<DateTime<Utc>>,
    incoming: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match (local, incoming) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, incoming) => incoming,
    }
}

/// Set union of `commit_shas`, appending in place.
///
/// Order the local copy already had is preserved, so merging does not reshuffle
/// a list a user may have just read. Like [`merge_ended_at`] this is
/// order-independent as a set operation, which is the property the write rules
/// depend on.
pub fn merge_commit_shas(local: &mut Vec<String>, incoming: &[String]) {
    let mut seen: BTreeSet<String> = local.iter().cloned().collect();
    for sha in incoming {
        if seen.insert(sha.clone()) {
            local.push(sha.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentKind, LineageId};

    #[test]
    fn detects_file_edit_artifact() {
        let mut c = Conversation::new(AgentKind::Cursor, "/tmp");
        c.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![crate::Artifact {
                kind: ArtifactKind::FileEdit,
                path: "src/main.rs".into(),
                blob_ref: None,
                content_hash: None,
                mime_type: None,
                preview_data_url: None,
                line_range: None,
                resolve: None,
            }],
        });
        assert!(conversation_modified_code(&c));
        assert_eq!(files_touched(&c), vec!["src/main.rs"]);
    }

    #[test]
    fn detects_tool_call_edit() {
        let mut c = Conversation::new(AgentKind::Claude, "/tmp");
        c.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![crate::ToolCall {
                id: "tc1".into(),
                name: "edit_file".into(),
                arguments: r#"{"path":"src/lib.rs"}"#.into(),
                result: None,
                target: None,
            }],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        assert!(turn_modified_code(&c.turns[0]));
        assert_eq!(files_touched(&c), vec!["src/lib.rs"]);
    }

    #[test]
    fn display_title_prefers_vendor_summary() {
        let mut c = Conversation::new(AgentKind::Claude, "/tmp");
        c.metadata.insert(
            SESSION_SUMMARY_KEY.into(),
            serde_json::Value::String("Lineage RLS audit".into()),
        );
        assert_eq!(display_title(&c), "Lineage RLS audit");
    }

    #[test]
    fn display_title_unwraps_agent_and_strips_slash_command_xml() {
        let mut c = Conversation::new(AgentKind::Claude, "/tmp");
        c.metadata.insert(
            SESSION_SUMMARY_KEY.into(),
            serde_json::Value::String("claude (<command-message>prime</command-message>)".into()),
        );
        assert_eq!(display_title(&c), "prime");
    }

    #[test]
    fn display_title_skips_slash_command_and_uses_the_opening_ask() {
        let mut c = Conversation::new(AgentKind::Claude, "/tmp");
        c.metadata.insert(
            SESSION_SUMMARY_KEY.into(),
            serde_json::Value::String("claude (<command-name>/model</command-name>)".into()),
        );
        c.turns.push(Turn {
            id: LineageId::new(),
            role: Role::User,
            content: "Add RLS to the shares table".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        assert_eq!(display_title(&c), "Add RLS to the shares table");
    }

    #[test]
    fn display_title_skips_cursor_chrome() {
        let mut c = Conversation::new(AgentKind::Cursor, "/tmp");
        c.id = LineageId::from("01KWQPW170F48DSH8M4988VZTT");
        c.metadata.insert(
            SESSION_SUMMARY_KEY.into(),
            serde_json::Value::String(
                "cursor (<timestamp>Friday, Jul 10, 2026, 11:28 PM (UTC+1)</timestamp>)".into(),
            ),
        );
        assert_eq!(display_title(&c), "01KWQPW1…");
    }

    #[test]
    fn humanize_text_strips_harness_chrome() {
        assert_eq!(
            humanize_text("<timestamp>Sunday, Aug 23, 2026, 6:02 PM (UTC+1)</timestamp>"),
            ""
        );
        assert_eq!(
            humanize_text("<user_query>apply consistent styling</user_query>"),
            "apply consistent styling"
        );
    }

    #[test]
    fn architecture_summary_includes_title_and_files() {
        let mut c = Conversation::new(AgentKind::Cursor, "/tmp");
        c.turns.push(Turn {
            id: LineageId::new(),
            role: Role::User,
            content: "Add caching layer".into(),
            tool_calls: vec![],
            model: Some("gpt-4".into()),
            timestamp: None,
            artifacts: vec![],
        });
        let summary = generate_architecture_summary(&c);
        assert!(summary.contains("caching"));
        assert!(summary.contains("gpt-4"));
    }

    #[test]
    fn enriched_body_includes_identifiers_from_tool_calls_and_edits() {
        // The dogfood failure: an identifier that appears only in a tool call
        // or an edit snippet, never in prose. Prose-only indexing missed it.
        let mut c = Conversation::new(AgentKind::Claude, "/repo");
        c.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: "Wiring the command up.".into(),
            tool_calls: vec![crate::ToolCall {
                id: "t1".into(),
                name: "Bash".into(),
                arguments: r#"{"file_path": "src/rebuild_index.rs"}"#.into(),
                result: None,
                target: None,
            }],
            model: None,
            timestamp: None,
            artifacts: vec![crate::Artifact {
                kind: ArtifactKind::FileEdit,
                path: "src/main.rs".into(),
                blob_ref: None,
                content_hash: None,
                mime_type: None,
                preview_data_url: None,
                line_range: None,
                resolve: Some(crate::ArtifactResolve {
                    strategy: crate::ResolveStrategy::OldString,
                    old_string: Some("fn rebuild()".into()),
                    new_string: Some("fn rebuild_index()".into()),
                    patch: None,
                }),
            }],
        });

        let body = enriched_indexable_body(&c);
        assert!(body.contains("Wiring the command up."));
        assert!(body.contains("Bash"));
        assert!(body.contains("src/rebuild_index.rs"));
        assert!(body.contains("src/main.rs"));
        assert!(body.contains("fn rebuild_index()"));
        assert!(body.contains("fn rebuild()"));
    }

    #[test]
    fn enriched_body_skips_empty_turns() {
        let mut c = Conversation::new(AgentKind::Claude, "/repo");
        c.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        assert_eq!(enriched_indexable_body(&c), "");
    }

    fn user_turn(content: &str) -> Turn {
        Turn {
            id: LineageId::new(),
            role: Role::User,
            content: content.into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        }
    }

    fn assistant_turn(content: &str) -> Turn {
        Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: content.into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        }
    }

    #[test]
    fn chunks_group_each_user_turn_with_its_replies() {
        let mut c = Conversation::new(AgentKind::Claude, "/repo");
        c.turns.push(user_turn("add caching"));
        c.turns.push(assistant_turn("done, edited cache.rs"));
        c.turns.push(user_turn("now add metrics"));
        c.turns.push(assistant_turn("added metrics.rs"));

        let chunks = session_chunks(&c, DEFAULT_CHUNK_MAX_CHARS);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].text.contains("add caching"));
        assert!(chunks[0].text.contains("cache.rs"));
        assert!(chunks[1].text.contains("add metrics"));
        assert!(chunks[1].text.contains("metrics.rs"));
    }

    #[test]
    fn oversized_group_splits_on_turn_boundary() {
        let mut c = Conversation::new(AgentKind::Claude, "/repo");
        c.turns.push(user_turn("start"));
        // Two assistant turns that together exceed a small budget must split
        // between turns, never within one.
        c.turns.push(assistant_turn(&"a".repeat(30)));
        c.turns.push(assistant_turn(&"b".repeat(30)));

        let chunks = session_chunks(&c, 40);
        assert!(chunks.len() >= 2);
        // No chunk mixes the two large turns' distinct content past the budget.
        assert!(chunks.iter().any(|c| c.text.contains(&"a".repeat(30))));
        assert!(chunks.iter().any(|c| c.text.contains(&"b".repeat(30))));
    }

    #[test]
    fn empty_session_has_no_chunks() {
        let c = Conversation::new(AgentKind::Claude, "/repo");
        assert!(session_chunks(&c, DEFAULT_CHUNK_MAX_CHARS).is_empty());
    }

    #[test]
    fn non_salient_turns_contribute_no_chunk_text() {
        let mut c = Conversation::new(AgentKind::Claude, "/repo");
        c.turns.push(user_turn("add caching"));
        let mut tool_result = assistant_turn("");
        tool_result.role = Role::Tool;
        tool_result.content = "500 lines of build output about caching".into();
        c.turns.push(tool_result);

        let chunks = session_chunks(&c, DEFAULT_CHUNK_MAX_CHARS);
        assert_eq!(chunks.len(), 1);
        assert!(!chunks[0].text.contains("build output"));
    }

    #[test]
    fn chunk_anchor_prefers_the_user_turn_then_first_contributor() {
        let mut c = Conversation::new(AgentKind::Claude, "/repo");
        // Narration first, then the user turn: the user turn anchors its own
        // group (a user turn replaces a non-user anchor), while the pre-user
        // narration forms its own chunk anchored on itself (first contributor).
        c.turns.push(assistant_turn("warming up"));
        c.turns.push(user_turn("add caching"));
        c.turns.push(assistant_turn("working on it"));

        let chunks = session_chunks(&c, DEFAULT_CHUNK_MAX_CHARS);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].anchor_turn_id, c.turns[0].id);
        assert_eq!(chunks[1].anchor_turn_id, c.turns[1].id);
    }

    #[test]
    fn a_single_giant_turn_is_split_under_the_cap() {
        // The OOM cause: one turn's text larger than the whole budget must be
        // broken up, not emitted as one unbounded chunk.
        let mut c = Conversation::new(AgentKind::Claude, "/repo");
        c.turns.push(assistant_turn(&"x".repeat(10_000)));

        let chunks = session_chunks(&c, 500);
        assert!(chunks.len() >= 20);
        assert!(
            chunks.iter().all(|c| c.text.chars().count() <= 500),
            "no chunk may exceed the cap"
        );
    }

    #[test]
    fn edit_snippets_are_capped_so_a_whole_file_write_cannot_bloat_a_turn() {
        let mut c = Conversation::new(AgentKind::Claude, "/repo");
        c.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![crate::Artifact {
                kind: ArtifactKind::FileEdit,
                path: "src/big.rs".into(),
                blob_ref: None,
                content_hash: None,
                mime_type: None,
                preview_data_url: None,
                line_range: None,
                resolve: Some(crate::ArtifactResolve {
                    strategy: crate::ResolveStrategy::OldString,
                    old_string: None,
                    new_string: Some("Z".repeat(100_000)),
                    patch: None,
                }),
            }],
        });

        let text = turn_indexable_text(&c.turns[0]);
        // The path survives in full; the giant snippet is bounded.
        assert!(text.contains("src/big.rs"));
        assert!(
            text.chars().count() < 2000,
            "a 100k-char edit must not produce a 100k-char turn text"
        );
    }

    fn at(secs: i64) -> Option<DateTime<Utc>> {
        DateTime::from_timestamp(secs, 0)
    }

    #[test]
    fn ended_at_takes_the_later_of_two_known_times() {
        assert_eq!(merge_ended_at(at(100), at(200)), at(200));
        assert_eq!(merge_ended_at(at(200), at(100)), at(200));
    }

    /// The rule that stops a copy which knows less from erasing what another
    /// knows: unknown loses to known, in both argument orders.
    #[test]
    fn ended_at_never_regresses_a_known_time_to_unknown() {
        assert_eq!(merge_ended_at(at(100), None), at(100));
        assert_eq!(merge_ended_at(None, at(100)), at(100));
        assert_eq!(merge_ended_at(None, None), None);
    }

    /// Order-independence is the property the write rules rest on: whichever
    /// direction a merge runs, both sides land on the same value.
    #[test]
    fn merges_are_order_independent() {
        assert_eq!(
            merge_ended_at(at(100), at(200)),
            merge_ended_at(at(200), at(100))
        );

        let mut forward = vec!["a".to_string(), "b".to_string()];
        merge_commit_shas(&mut forward, &["b".to_string(), "c".to_string()]);
        let mut backward = vec!["b".to_string(), "c".to_string()];
        merge_commit_shas(&mut backward, &["a".to_string(), "b".to_string()]);

        let sorted = |mut v: Vec<String>| {
            v.sort();
            v
        };
        assert_eq!(sorted(forward), sorted(backward));
    }

    #[test]
    fn commit_shas_union_dedupes_and_keeps_local_order() {
        let mut local = vec!["c".to_string(), "a".to_string()];
        merge_commit_shas(&mut local, &["a".to_string(), "b".to_string()]);
        assert_eq!(
            local,
            vec!["c", "a", "b"],
            "existing order survives, new appends"
        );
    }
}
