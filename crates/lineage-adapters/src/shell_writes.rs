//! Recovering file-write provenance from shell commands (heredocs today).
//!
//! Agents frequently edit files by shelling out — `cat > f << EOF … EOF` — and
//! that write is invisible to tool-name matching: it arrives as one opaque Bash
//! `command` string. Parsing the transcript-recoverable cases (the full
//! post-image is literally in the command) lets these feed the normal
//! materialization path. Command text is untrusted data — this only pattern-
//! matches, never executes.

use lineage_core::ResolveStrategy;

/// A file write recovered from a shell command: path plus the post-image text
/// to anchor line objects on (per gap 11, the post-image is the anchor that
/// exists in the committed tree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellWrite {
    pub path: String,
    /// Post-edit content: whole file for a create, the appended chunk for an
    /// append (still a valid anchor — it exists verbatim in the result).
    pub new_string: String,
    pub strategy: ResolveStrategy,
}

/// Parse a shell command for recoverable file writes. Empty when the command
/// contains no recognized write (the caller keeps its terminal-command
/// fallback). Multiple heredocs in one command each yield a write.
pub fn parse_shell_writes(command: &str) -> Vec<ShellWrite> {
    let mut writes = Vec::new();
    let mut rest = command;
    while let Some((write, tail)) = next_heredoc_write(rest) {
        if let Some(write) = write {
            writes.push(write);
        }
        rest = tail;
    }
    writes
}

/// Finds the next `<<` heredoc and returns (recovered write or None if it is
/// not a file redirect, remaining input after the heredoc body). Returns None
/// for the whole call when no `<<` remains.
fn next_heredoc_write(input: &str) -> Option<(Option<ShellWrite>, &str)> {
    let marker_pos = input.find("<<")?;
    let (head, after_marker) = input.split_at(marker_pos);
    let after_marker = &after_marker[2..];

    let (delimiter, body_start) = parse_heredoc_delimiter(after_marker);
    let Some(delimiter) = delimiter else {
        // Malformed marker (e.g. a bare `<<`); skip past it so we don't loop.
        return Some((None, body_start));
    };

    let (body, tail) = split_heredoc_body(body_start, &delimiter);

    // The redirect and its target sit in the text *before* `<<`
    // (`cat > f <<EOF`). A heredoc feeding a command flag
    // (`git commit -m "$(cat <<EOF)"`) has no file redirect there — that is the
    // false-positive trap, so only a real `> file` / `>> file` counts.
    let Some((path, strategy)) = redirect_target(head) else {
        return Some((None, tail));
    };

    Some((
        Some(ShellWrite {
            path,
            new_string: body,
            strategy,
        }),
        tail,
    ))
}

/// After `<<`, read the delimiter word. Handles `<<EOF`, `<< EOF`, `<<'EOF'`,
/// `<<-EOF` (leading-tab variant). Returns (delimiter, rest after the delimiter
/// token through end of its line).
fn parse_heredoc_delimiter(after: &str) -> (Option<String>, &str) {
    let after = after.strip_prefix('-').unwrap_or(after);
    let after = after.trim_start_matches([' ', '\t']);
    let after = after.strip_prefix('$').unwrap_or(after); // `<<$'EOF'` — rare

    // Delimiter may be quoted; quoting only affects expansion, not our match.
    let (quote, body) = match after.chars().next() {
        Some(q @ ('\'' | '"')) => (Some(q), &after[1..]),
        _ => (None, after),
    };
    let end = match quote {
        Some(q) => body.find(q),
        None => body.find(|c: char| c.is_whitespace()),
    };
    let Some(end) = end else {
        return (None, after);
    };
    let delimiter = body[..end].to_string();
    // Skip the closing quote (if any) and the remainder of the opening line.
    let after_delim = &body[end + quote.map_or(0, |_| 1)..];
    let line_end = after_delim
        .find('\n')
        .map(|i| i + 1)
        .unwrap_or(after_delim.len());
    (Some(delimiter), &after_delim[line_end..])
}

/// Split the heredoc body at a line equal to the delimiter (leading tabs
/// tolerated for the `<<-` form). Returns (body, remaining input after the
/// closing delimiter line).
fn split_heredoc_body<'a>(input: &'a str, delimiter: &str) -> (String, &'a str) {
    let mut body_len = 0;
    for line in input.split_inclusive('\n') {
        if line.trim_end_matches('\n').trim_start_matches('\t') == delimiter {
            let after = &input[body_len + line.len()..];
            // Drop the trailing newline that precedes the closing delimiter so
            // the body is the file's content, not content + a spurious blank.
            let body = input[..body_len]
                .strip_suffix('\n')
                .unwrap_or(&input[..body_len]);
            return (body.to_string(), after);
        }
        body_len += line.len();
    }
    // No closing delimiter (truncated transcript): take what we have.
    (input.to_string(), "")
}

/// The file redirect target immediately before a heredoc marker, if any.
/// Both `>` (create) and `>>` (append) resolve as `FullFile`: the recovered
/// `new_string` is the post-image text (whole file, or the appended chunk),
/// and that text is the anchor regardless. A missing redirect means the
/// heredoc feeds a command/flag, not a file — the false-positive trap.
fn redirect_target(head: &str) -> Option<(String, ResolveStrategy)> {
    // Consider only the current command segment: heredocs feeding a flag look
    // like `… -m "$(cat ` — the `(` starts a subshell with no file redirect.
    let segment = head.rsplit(['(', ';', '&', '|']).next().unwrap_or(head);
    let bytes = segment.as_bytes();
    let mut i = 0;
    let mut last: Option<(String, ResolveStrategy)> = None;
    while i < bytes.len() {
        if bytes[i] == b'>' {
            let append = i + 1 < bytes.len() && bytes[i + 1] == b'>';
            let after = &segment[i + if append { 2 } else { 1 }..];
            let target = after.split_whitespace().next().unwrap_or("");
            if is_file_target(target) {
                last = Some((target.to_string(), ResolveStrategy::FullFile));
            }
            i += if append { 2 } else { 1 };
        } else {
            i += 1;
        }
    }
    last
}

/// A redirect target is a file when it is a plausible path — not a file
/// descriptor dup (`&1`), `/dev/*`, or empty.
fn is_file_target(target: &str) -> bool {
    !target.is_empty()
        && !target.starts_with('&')
        && !target.starts_with("/dev/")
        && target != "/dev/null"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_cat_create() {
        let cmd = "cat > src/lib.rs << 'EOF'\npub fn f() {}\nEOF";
        let writes = parse_shell_writes(cmd);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].path, "src/lib.rs");
        assert_eq!(writes[0].new_string, "pub fn f() {}");
        assert_eq!(writes[0].strategy, ResolveStrategy::FullFile);
    }

    #[test]
    fn recovers_cat_append() {
        let cmd = "cat >> CHANGELOG.md <<EOF\n- new entry\nEOF";
        let writes = parse_shell_writes(cmd);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].path, "CHANGELOG.md");
        assert_eq!(writes[0].new_string, "- new entry");
    }

    #[test]
    fn rejects_heredoc_into_commit_message() {
        // The false-positive trap: heredoc feeds a flag, not a file.
        let cmd = "git commit -m \"$(cat <<'EOF'\nfeat: thing\nEOF\n)\"";
        assert!(parse_shell_writes(cmd).is_empty());
    }

    #[test]
    fn rejects_heredoc_into_pr_body() {
        let cmd = "gh pr create --body \"$(cat <<EOF\n## What\nchanges\nEOF\n)\"";
        assert!(parse_shell_writes(cmd).is_empty());
    }

    #[test]
    fn ignores_stdin_heredoc_with_no_redirect() {
        // `python3 - <<EOF` reads a script on stdin; not a file write.
        let cmd = "python3 - <<'EOF'\nprint('hi')\nEOF";
        assert!(parse_shell_writes(cmd).is_empty());
    }

    #[test]
    fn handles_multiple_heredocs() {
        let cmd = "cat > a.txt <<EOF\nA\nEOF\ncat > b.txt <<EOF\nB\nEOF";
        let writes = parse_shell_writes(cmd);
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].path, "a.txt");
        assert_eq!(writes[1].path, "b.txt");
    }

    #[test]
    fn tolerates_leading_tab_dash_form() {
        let cmd = "cat > f.txt <<-EOF\n\tindented\nEOF";
        let writes = parse_shell_writes(cmd);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].path, "f.txt");
    }

    #[test]
    fn empty_when_no_heredoc() {
        assert!(parse_shell_writes("cargo build && git status").is_empty());
    }
}
