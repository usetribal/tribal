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
    /// append, or the replacement text for an `OldString` edit — all valid
    /// anchors (they exist verbatim in the result).
    pub new_string: String,
    /// Present only for `OldString` edits (python `.replace(old, new)`): the
    /// pre-edit text, the resolver's fallback anchor.
    pub old_string: Option<String>,
    pub strategy: ResolveStrategy,
}

/// Parse a shell command for recoverable file writes — heredoc redirects
/// (`FullFile`) and python literal `.replace(old, new)` edits (`OldString`).
/// Empty when nothing is recognized (the caller keeps its terminal-command
/// fallback).
pub fn parse_shell_writes(command: &str) -> Vec<ShellWrite> {
    let mut writes = Vec::new();
    let mut rest = command;
    while let Some((write, tail)) = next_heredoc_write(rest) {
        if let Some(write) = write {
            writes.push(write);
        } else {
            // A `python3 - <<EOF` heredoc has no file redirect (so
            // `next_heredoc_write` yields None), but its body may hold replaces.
            let body = &rest[..rest.len() - tail.len()];
            writes.extend(parse_python_replaces(body));
        }
        rest = tail;
    }
    // A `python3 -c "…"` command has no heredoc at all; scan the whole thing.
    if writes.is_empty() {
        writes.extend(parse_python_replaces(command));
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
            old_string: None,
            strategy,
        }),
        tail,
    ))
}

/// Recognize the `open(p).read()` → `.replace(old, new)` → `open(p,'w').write`
/// pattern in a python script body and emit one `OldString` edit per replace.
/// Only literal (parsed-from-source) `old`/`new` are recoverable — the regex
/// variant (`re.compile`/`.subn`) is Tier 2 and deliberately not handled here.
fn parse_python_replaces(body: &str) -> Vec<ShellWrite> {
    // Must actually write a file back; a read-only inspect script is not a write.
    if !body.contains(".replace(")
        || body.contains("re.compile")
        || body.contains(".subn(")
        || py::has_regex_sub(body)
    {
        return Vec::new();
    }
    let Some(path) = py::find_write_path(body) else {
        return Vec::new();
    };
    let vars = py::collect_string_assignments(body);

    let mut writes = Vec::new();
    for (old_expr, new_expr) in py::find_replace_calls(body) {
        // Both arguments must resolve to literals (directly or via a variable);
        // a computed replacement is not transcript-recoverable.
        let (Some(old), Some(new)) = (
            resolve_literal(&old_expr, &vars),
            resolve_literal(&new_expr, &vars),
        ) else {
            continue;
        };
        if old.is_empty() {
            continue;
        }
        writes.push(ShellWrite {
            path: path.clone(),
            new_string: new,
            old_string: Some(old),
            strategy: ResolveStrategy::OldString,
        });
    }
    writes
}

/// A `.replace` argument is a string literal or a variable bound earlier.
fn resolve_literal(expr: &str, vars: &std::collections::HashMap<String, String>) -> Option<String> {
    let expr = expr.trim();
    if let Some(lit) = py::parse_string_literal(expr) {
        return Some(lit);
    }
    vars.get(expr).cloned()
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

    #[test]
    fn recovers_python_literal_replace_inline() {
        let cmd = "python3 - <<'EOF'\np = 'src/auth.rs'\ns = open(p).read()\nopen(p, 'w').write(s.replace('fn old() {}', 'fn new() {}'))\nEOF";
        let writes = parse_shell_writes(cmd);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].path, "src/auth.rs");
        assert_eq!(writes[0].strategy, ResolveStrategy::OldString);
        assert_eq!(writes[0].old_string.as_deref(), Some("fn old() {}"));
        assert_eq!(writes[0].new_string, "fn new() {}");
    }

    #[test]
    fn recovers_python_replace_via_variables_and_triple_quotes() {
        // The dominant corpus shape: p/old/new bound to triple-quoted literals,
        // an intervening assert, two-step read/replace/write.
        let cmd = "python3 - <<'PYEOF'\np = 'docs/x.md'\ns = open(p).read()\nold = \"\"\"line one\nline two\n\"\"\"\nnew = \"\"\"line one\nline two changed\n\"\"\"\nassert old in s\ns = s.replace(old, new)\nopen(p, 'w').write(s)\nPYEOF";
        let writes = parse_shell_writes(cmd);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].path, "docs/x.md");
        assert_eq!(
            writes[0].old_string.as_deref(),
            Some("line one\nline two\n")
        );
        assert_eq!(writes[0].new_string, "line one\nline two changed\n");
    }

    #[test]
    fn recovers_multiple_replaces_on_one_file() {
        let cmd = "python3 - <<'EOF'\np='f.md'\ns=open(p).read()\ns=s.replace('a','A')\ns=s.replace('b','B')\nopen(p,'w').write(s)\nEOF";
        let writes = parse_shell_writes(cmd);
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].old_string.as_deref(), Some("a"));
        assert_eq!(writes[1].new_string, "B");
    }

    #[test]
    fn ignores_python_regex_substitution() {
        // Tier 2, not us: the replacement is computed by the regex engine.
        let cmd = "python3 - <<'EOF'\nimport re\np='f.rs'\nt=open(p).read()\nt2,n=re.compile(r'a+').subn('X',t)\nopen(p,'w').write(t2)\nEOF";
        assert!(parse_shell_writes(cmd).is_empty());
    }

    #[test]
    fn ignores_read_only_python_inspect() {
        // No write-back — inspection, not authorship.
        let cmd = "python3 - <<'EOF'\ns=open('f.rs').read()\nprint(s.replace('a','b'))\nEOF";
        assert!(parse_shell_writes(cmd).is_empty());
    }

    #[test]
    fn python_c_string_form() {
        let cmd = "python3 -c \"p='f'; s=open(p).read(); open(p,'w').write(s.replace('x','y'))\"";
        let writes = parse_shell_writes(cmd);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].old_string.as_deref(), Some("x"));
    }
}

/// Minimal python-source scanning for the literal-replace pattern. Not a python
/// parser — just enough to pull string literals and the replace/write shape out
/// of the small scripts agents inline. All parsing, never evaluation.
mod py {
    use std::collections::HashMap;

    /// The file path written back: the first arg of `open(p, 'w')`, resolved
    /// through string assignments. None if the script never writes.
    pub fn find_write_path(body: &str) -> Option<String> {
        let idx = body.find("'w')").or_else(|| body.find("\"w\")"))?;
        let before = &body[..idx];
        let open_at = before.rfind("open(")?;
        let first_arg = before[open_at + 5..].split(',').next()?.trim();
        let vars = collect_string_assignments(body);
        parse_string_literal(first_arg).or_else(|| vars.get(first_arg).cloned())
    }

    /// True if the script does a regex substitution (Tier 2, not us).
    pub fn has_regex_sub(body: &str) -> bool {
        body.contains("re.sub(")
    }

    /// `name = <string literal>` bindings, so a `.replace(old, new)` referring
    /// to `old`/`new`/`p` by name resolves.
    pub fn collect_string_assignments(body: &str) -> HashMap<String, String> {
        let mut map = HashMap::new();
        let mut i = 0;
        while i < body.len() {
            if let Some((name, after_eq)) = assignment_at(body, i) {
                if let Some((lit, consumed)) = parse_string_literal_prefix(&body[after_eq..]) {
                    map.entry(name).or_insert(lit);
                    i = after_eq + consumed;
                    continue;
                }
            }
            i += 1;
        }
        map
    }

    /// If position `i` begins `<ident> = `, return (ident, index past `=`).
    fn assignment_at(body: &str, i: usize) -> Option<(String, usize)> {
        let bytes = body.as_bytes();
        // ident starts at a statement boundary — newline, `;`, whitespace, or
        // the quote that opens a `python3 -c "…"` script.
        if i > 0 && !matches!(bytes[i - 1], b'\n' | b';' | b' ' | b'\t' | b'"' | b'\'') {
            return None;
        }
        let mut j = i;
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
            j += 1;
        }
        if j == i || body[i..].chars().next()?.is_ascii_digit() {
            return None;
        }
        let name = &body[i..j];
        let mut k = j;
        while k < bytes.len() && bytes[k] == b' ' {
            k += 1;
        }
        if k >= bytes.len() || bytes[k] != b'=' {
            return None;
        }
        if k + 1 < bytes.len() && bytes[k + 1] == b'=' {
            return None; // `==`, not assignment
        }
        let mut after = k + 1;
        while after < bytes.len() && bytes[after] == b' ' {
            after += 1;
        }
        Some((name.to_string(), after))
    }

    /// Every `.replace(<argA>, <argB>)` — raw argument expressions for the
    /// caller to resolve.
    pub fn find_replace_calls(body: &str) -> Vec<(String, String)> {
        let mut calls = Vec::new();
        let mut from = 0;
        while let Some(rel) = body[from..].find(".replace(") {
            let start = from + rel + ".replace(".len();
            if let Some((a, b, end)) = two_args(&body[start..]) {
                calls.push((a, b));
                from = start + end;
            } else {
                from = start;
            }
        }
        calls
    }

    /// Two comma-separated args up to the matching `)`, respecting string
    /// literals so commas inside them don't split. Returns (argA, argB, index
    /// past the closing paren).
    fn two_args(s: &str) -> Option<(String, String, usize)> {
        let bytes = s.as_bytes();
        let mut i = 0;
        let mut args: Vec<String> = Vec::new();
        let mut cur_start = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\'' | b'"' => {
                    let (_, consumed) = parse_string_literal_prefix(&s[i..])?;
                    i += consumed;
                }
                b',' if args.is_empty() => {
                    args.push(s[cur_start..i].trim().to_string());
                    cur_start = i + 1;
                    i += 1;
                }
                b')' => {
                    args.push(s[cur_start..i].trim().to_string());
                    return (args.len() >= 2).then(|| (args[0].clone(), args[1].clone(), i + 1));
                }
                _ => i += 1,
            }
        }
        None
    }

    /// Parse a python string literal that occupies the entire expression.
    pub fn parse_string_literal(expr: &str) -> Option<String> {
        let (lit, consumed) = parse_string_literal_prefix(expr)?;
        expr[consumed..].trim().is_empty().then_some(lit)
    }

    /// Parse a python string literal at the start of `s`; returns (value, bytes
    /// consumed). Handles triple-quoted and single-line `'`/`"` with
    /// `\n`/`\t`/`\\`/`\"`/`\'` unescaping; `r`/`b` prefixes accepted, `f`
    /// (computed) rejected.
    pub fn parse_string_literal_prefix(s: &str) -> Option<(String, usize)> {
        let bytes = s.as_bytes();
        let mut off = 0;
        while off < bytes.len() && matches!(bytes[off], b'r' | b'b' | b'R' | b'B') {
            off += 1;
        }
        if off < bytes.len() && matches!(bytes[off], b'f' | b'F') {
            return None;
        }
        let rest = &s[off..];
        for triple in ["\"\"\"", "'''"] {
            if let Some(inner) = rest.strip_prefix(triple) {
                let end = inner.find(triple)?;
                return Some((
                    inner[..end].to_string(),
                    off + triple.len() + end + triple.len(),
                ));
            }
        }
        let quote = match rest.as_bytes().first() {
            Some(&q @ (b'\'' | b'"')) => q,
            _ => return None,
        };
        let inner = &rest[1..];
        let ib = inner.as_bytes();
        let mut out = String::new();
        let mut i = 0;
        while i < ib.len() {
            let c = ib[i];
            if c == b'\\' && i + 1 < ib.len() {
                match ib[i + 1] {
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'\\' => out.push('\\'),
                    b'\'' => out.push('\''),
                    b'"' => out.push('"'),
                    other => {
                        out.push('\\');
                        out.push(other as char);
                    }
                }
                i += 2;
                continue;
            }
            if c == quote {
                return Some((out, off + 1 + i + 1));
            }
            let ch = inner[i..].chars().next()?;
            out.push(ch);
            i += ch.len_utf8();
        }
        None
    }
}
