//! The rules-only plan dispatcher (plan v0). It is a plan *selector*, not a plan
//! *builder*: it routes a free-text query to one of the two canned plans and
//! nothing else (LLM plan-building is the server-side story). The rule is safe by
//! construction — a query routes to the line-anchored temporal plan only when a
//! path-like token it names actually exists in the corpus or the working tree, so
//! a wrong guess degrades to the fused plan (which itself degrades to
//! honest-nothing), never to a wrong answer.
//!
//! The dispatcher lives here because routing is retrieval logic; the CLI only
//! wires the chosen plan to its runner.

use std::path::Path;

use lineage_search::LineageIndex;

/// Which canned plan a query routes to. The enum is the whole plan catalogue at
/// v0 — two entries — and every free-text query lands on exactly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// FTS ∥ dense → RRF → materialize verbatim turns. The default; degrades to
    /// honest-nothing.
    Fused,
    /// A file[:line] anchor → line_objects/ancestry → time-ordered turns.
    Temporal,
}

impl Plan {
    pub fn as_str(self) -> &'static str {
        match self {
            Plan::Fused => "fused",
            Plan::Temporal => "temporal",
        }
    }
}

/// The routing decision, made loggable and testable. `matched_anchor` is the
/// `path` or `path:line` the temporal plan will anchor on (only set when routing
/// temporal); `signals` names every rule that fired, so the log records *why* a
/// query went where it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecision {
    pub plan: Plan,
    pub matched_anchor: Option<String>,
    pub signals: Vec<String>,
}

/// A candidate token pulled from the query text, tagged with the shape that
/// produced it. Only path-shaped candidates can route temporal; identifier
/// candidates are extracted for the log (the fused leg's tokenizer already
/// handles their ranking) but never anchor a walk.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    text: String,
    line: Option<u32>,
    kind: CandidateKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateKind {
    /// Contains `/` or ends in a known source extension — a thing that can be a
    /// file path and therefore a line anchor.
    Path,
    /// snake_case / kebab-case / CamelCase / dotted.name — a symbol, not a path.
    Identifier,
}

/// Route a free-text query to a canned plan. `index` is consulted for corpus
/// hit-tests (the indexed lookups that make the rule safe); `workdir` lets a path
/// that exists on disk but is not yet indexed still anchor a walk (a freshly
/// written file the agent is asking about). Deterministic and cheap: tokenize,
/// then at most a few indexed lookups — no model, microseconds.
pub fn route(query: &str, index: &LineageIndex, workdir: &Path) -> RouteDecision {
    let candidates = extract_candidates(query);

    // Path candidates are tried in text order; the first that hit-tests wins, so
    // a query naming one real file and one typo routes on the real one.
    for candidate in candidates.iter().filter(|c| c.kind == CandidateKind::Path) {
        let Some(mut signals) = path_hit_signals(&candidate.text, index, workdir) else {
            continue;
        };
        signals.insert(0, "path-token".to_string());
        if candidate.line.is_some() {
            signals.push("line-anchor".to_string());
        }
        return RouteDecision {
            plan: Plan::Temporal,
            matched_anchor: Some(anchor_string(candidate)),
            signals,
        };
    }

    // No path hit-tested. A bare identifier is a ranking concern the fused leg
    // already owns (its tokenizer preserves identifiers), so identifier presence
    // is recorded but does not change the route — there is no line to walk.
    let mut signals = Vec::new();
    if candidates.iter().any(|c| c.kind == CandidateKind::Path) {
        signals.push("path-token-no-hit".to_string());
    }
    if candidates
        .iter()
        .any(|c| c.kind == CandidateKind::Identifier)
    {
        signals.push("identifier-token".to_string());
    }
    if signals.is_empty() {
        signals.push("prose".to_string());
    }
    RouteDecision {
        plan: Plan::Fused,
        matched_anchor: None,
        signals,
    }
}

fn anchor_string(candidate: &Candidate) -> String {
    match candidate.line {
        Some(line) => format!("{}:{}", candidate.text, line),
        None => candidate.text.clone(),
    }
}

/// The hit-test: which corpus/tree signals fired for a path candidate, or `None`
/// if it exists nowhere (so it must not route temporal). Checked cheapest-first —
/// two indexed lookups then a stat — and every hit source is named so the log
/// distinguishes an indexed anchor from a working-tree-only one.
fn path_hit_signals(path: &str, index: &LineageIndex, workdir: &Path) -> Option<Vec<String>> {
    let mut signals = Vec::new();
    if index
        .line_objects_for_file(path)
        .is_ok_and(|rows| !rows.is_empty())
    {
        signals.push("line-objects-hit".to_string());
    }
    if index
        .sessions_for_file(path)
        .is_ok_and(|sessions| !sessions.is_empty())
    {
        signals.push("session-files-hit".to_string());
    }
    // A path that exists on disk but is not indexed still has a HEAD line object
    // the temporal plan can anchor on, so the working tree is a valid hit source.
    if workdir.join(path).is_file() {
        signals.push("working-tree-hit".to_string());
    }
    (!signals.is_empty()).then_some(signals)
}

/// Known source extensions that make a bare token (no `/`) path-shaped. A short
/// closed list, not NLP: a word ending in one of these is treated as a filename
/// so `commands.rs` routes even without a directory. Extension-only tokens still
/// pass through the hit-test, so a made-up `foo.rs` stays fused.
const PATH_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "rb", "c", "cc", "cpp", "h", "hpp", "md",
    "toml", "json", "yaml", "yml", "sql", "sh",
];

/// Pull path-like and identifier-like candidates out of the query, in order of
/// appearance. Deliberately simple: split on whitespace, strip surrounding
/// punctuation, classify each word by shape. No stemming, no NLP.
fn extract_candidates(query: &str) -> Vec<Candidate> {
    query.split_whitespace().filter_map(classify_word).collect()
}

fn classify_word(raw: &str) -> Option<Candidate> {
    let word = raw.trim_matches(|c: char| !is_token_char(c));
    if word.is_empty() {
        return None;
    }
    let (path, line) = split_line_suffix(word);
    if is_path_shaped(path) {
        return Some(Candidate {
            text: path.to_string(),
            line,
            kind: CandidateKind::Path,
        });
    }
    // A `path:line` split that produced no path shape is not an anchor; fall back
    // to classifying the whole word (a `dotted.name` identifier, say).
    if is_identifier_shaped(word) {
        return Some(Candidate {
            text: word.to_string(),
            line: None,
            kind: CandidateKind::Identifier,
        });
    }
    None
}

/// Characters that may appear inside a path or identifier token; everything else
/// (quotes, commas, sentence punctuation) is stripped from the token's edges.
/// `:` stays so a `path:line` anchor survives trimming.
fn is_token_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':')
}

/// Split a trailing `:<number>` line anchor off a token. Only a numeric suffix
/// counts (a `foo:bar` is not an anchor), matching the CLI's `parse_file_anchor`.
fn split_line_suffix(word: &str) -> (&str, Option<u32>) {
    if let Some((path, line_str)) = word.rsplit_once(':') {
        if let Ok(line) = line_str.parse::<u32>() {
            return (path, Some(line));
        }
    }
    (word, None)
}

/// A token is path-shaped if it names a directory (`/`) or ends in a known source
/// extension. A bare dotted word like `rebuild.index` is not path-shaped unless
/// its final segment is a known extension — otherwise every `a.b` identifier
/// would masquerade as a file.
fn is_path_shaped(word: &str) -> bool {
    if word.contains('/') {
        return true;
    }
    word.rsplit_once('.')
        .is_some_and(|(_, ext)| PATH_EXTENSIONS.contains(&ext))
}

/// snake_case / kebab-case / CamelCase / dotted.name — a multi-part symbol. A
/// single lowercase word (`rebuild`) is not an identifier candidate: it carries
/// no structural signal over ordinary prose and would tag most of a sentence.
fn is_identifier_shaped(word: &str) -> bool {
    let has_separator = word.contains('_') || word.contains('-') || word.contains('.');
    let is_camel = has_inner_uppercase(word);
    (has_separator || is_camel) && word.chars().any(|c| c.is_alphanumeric())
}

/// An uppercase letter after a lowercase one — the CamelCase signal. A leading
/// capital alone (a sentence-start word) does not count.
fn has_inner_uppercase(word: &str) -> bool {
    word.chars()
        .zip(word.chars().skip(1))
        .any(|(prev, cur)| prev.is_lowercase() && cur.is_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_path_tokens_by_slash_and_extension() {
        let c = extract_candidates("why did oss/crates/commands.rs and lib.rs get big");
        let paths: Vec<_> = c
            .iter()
            .filter(|c| c.kind == CandidateKind::Path)
            .map(|c| c.text.as_str())
            .collect();
        assert_eq!(paths, vec!["oss/crates/commands.rs", "lib.rs"]);
    }

    #[test]
    fn extracts_line_anchor_from_path_token() {
        let c = extract_candidates("what changed around src/lib.rs:10 lately");
        let path = c.iter().find(|c| c.kind == CandidateKind::Path).unwrap();
        assert_eq!(path.text, "src/lib.rs");
        assert_eq!(path.line, Some(10));
    }

    #[test]
    fn classifies_identifiers_but_not_bare_words() {
        // snake_case, kebab-case, CamelCase, dotted.name are identifiers.
        for id in ["rebuild_index", "rebuild-index", "FtsRetriever", "std.env"] {
            let c = extract_candidates(id);
            assert_eq!(c.len(), 1, "{id} should classify");
            assert_eq!(c[0].kind, CandidateKind::Identifier, "{id}");
        }
        // A plain word carries no structural signal — not a candidate.
        assert!(extract_candidates("rebuild the whole thing").is_empty());
    }

    #[test]
    fn dotted_identifier_is_not_a_path_unless_known_extension() {
        // A dotted word whose suffix is not a source extension stays an
        // identifier, not a phantom file.
        let c = extract_candidates("config.settings changed");
        assert_eq!(c[0].kind, CandidateKind::Identifier);
        // A known extension makes it path-shaped.
        let c = extract_candidates("config.toml changed");
        assert_eq!(c[0].kind, CandidateKind::Path);
    }

    #[test]
    fn strips_surrounding_punctuation() {
        let c = extract_candidates("\"commands.rs\", (lib.rs)?");
        let paths: Vec<_> = c.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(paths, vec!["commands.rs", "lib.rs"]);
    }

    // The canned prompt test set: a fixed list of prompts routed against a real
    // tempfile corpus, so the hit-tests are real (a mocked index could not tell
    // an indexed file from a phantom one). This is the plan's "routes as
    // expected" acceptance surface.
    mod corpus {
        use super::*;
        use lineage_core::{
            AgentKind, Artifact, ArtifactKind, ArtifactResolve, Conversation, LineObject,
            LineageId, ResolveStrategy, Role, Turn,
        };
        use lineage_git::{open_repo, persist_conversation, write_line_object};

        /// A repo where `commands.rs` is a real, indexed, line-object-attributed
        /// file (it hit-tests every way), plus an untracked `scratch.rs` on disk
        /// only (working-tree hit, no index rows). Nothing named `missing.rs`
        /// exists anywhere.
        fn seeded_repo() -> (tempfile::TempDir, LineageIndex) {
            let dir = tempfile::tempdir().unwrap();
            std::process::Command::new("git")
                .args(["init"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            let repo = open_repo(dir.path()).unwrap();

            let mut conv = Conversation::new(AgentKind::Claude, dir.path().display().to_string());
            conv.turns.push(Turn {
                id: LineageId::new(),
                role: Role::User,
                content: "why did commands.rs get so big".into(),
                tool_calls: vec![],
                model: None,
                timestamp: None,
                artifacts: vec![],
            });
            conv.turns.push(Turn {
                id: LineageId::new(),
                role: Role::Assistant,
                content: String::new(),
                tool_calls: vec![lineage_core::ToolCall {
                    id: "t".into(),
                    name: "Edit".into(),
                    arguments: r#"{"file_path": "commands.rs"}"#.into(),
                    result: None,
                }],
                model: None,
                timestamp: None,
                artifacts: vec![Artifact {
                    kind: ArtifactKind::FileEdit,
                    path: "commands.rs".into(),
                    blob_ref: None,
                    content_hash: None,
                    mime_type: None,
                    preview_data_url: None,
                    line_range: None,
                    resolve: Some(ArtifactResolve {
                        strategy: ResolveStrategy::OldString,
                        old_string: None,
                        new_string: Some("fn cmd() {}".into()),
                        patch: None,
                    }),
                }],
            });
            persist_conversation(repo.inner(), &conv).unwrap();
            let commit = commit_file(dir.path(), "commands.rs", "fn cmd() {}\n");

            let obj = LineObject::new(
                "commands.rs",
                [1, 1],
                commit,
                conv.id.clone(),
                conv.turns[0].id.clone(),
                lineage_core::Confidence::Exact,
            );
            write_line_object(repo.inner(), &obj).unwrap();

            // An on-disk-only file: no index rows, but a working-tree hit lets
            // the temporal plan anchor its HEAD line object.
            std::fs::write(dir.path().join("scratch.rs"), "fn s() {}\n").unwrap();

            let index = LineageIndex::open(dir.path().join(".git/lineage/index.db")).unwrap();
            index.index_conversation(&conv).unwrap();
            index
                .populate_line_tables(repo.inner(), &mut |_, _| {})
                .unwrap();
            (dir, index)
        }

        fn commit_file(dir: &std::path::Path, path: &str, contents: &str) -> String {
            std::fs::write(dir.join(path), contents).unwrap();
            for args in [
                vec!["add", path],
                vec![
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "user.name=t",
                    "commit",
                    "-m",
                    "add",
                ],
            ] {
                std::process::Command::new("git")
                    .args(&args)
                    .current_dir(dir)
                    .output()
                    .unwrap();
            }
            let out = std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(dir)
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        }

        #[test]
        fn canned_prompts_route_as_expected() {
            let (dir, index) = seeded_repo();
            let workdir = dir.path();

            // (prompt, expected plan, expected anchor)
            let cases: &[(&str, Plan, Option<&str>)] = &[
                // A prompt naming a real indexed file → temporal on that file.
                (
                    "why did commands.rs get so big",
                    Plan::Temporal,
                    Some("commands.rs"),
                ),
                // A path that exists nowhere in lineage or on disk → fused.
                ("what happened in missing.rs", Plan::Fused, None),
                // Pure-intent prose (no path, no identifier) → fused.
                ("how did we implement the CLI login auth", Plan::Fused, None),
                // Identifier-only prompt: identifiers are not paths → fused.
                (
                    "what's the difference between rebuild and rebuild-index",
                    Plan::Fused,
                    None,
                ),
                // An explicit path:line whose file hit-tests → temporal, line anchor.
                (
                    "what changed around commands.rs:1",
                    Plan::Temporal,
                    Some("commands.rs:1"),
                ),
                // A working-tree-only file (not indexed) still anchors temporal.
                ("why is scratch.rs here", Plan::Temporal, Some("scratch.rs")),
            ];

            for (prompt, want_plan, want_anchor) in cases {
                let decision = route(prompt, &index, workdir);
                assert_eq!(decision.plan, *want_plan, "plan for {prompt:?}");
                assert_eq!(
                    decision.matched_anchor.as_deref(),
                    *want_anchor,
                    "anchor for {prompt:?}"
                );
                assert!(
                    !decision.signals.is_empty(),
                    "signals recorded for {prompt:?}"
                );
            }
        }

        #[test]
        fn temporal_route_names_its_hit_sources() {
            let (dir, index) = seeded_repo();
            let decision = route("why did commands.rs get so big", &index, dir.path());
            assert_eq!(decision.plan, Plan::Temporal);
            assert!(decision.signals.contains(&"path-token".to_string()));
            assert!(decision.signals.contains(&"line-objects-hit".to_string()));
        }

        #[test]
        fn identifier_only_records_the_identifier_signal() {
            let (dir, index) = seeded_repo();
            let decision = route(
                "difference between rebuild-index and rebuild_index",
                &index,
                dir.path(),
            );
            assert_eq!(decision.plan, Plan::Fused);
            assert!(decision.signals.contains(&"identifier-token".to_string()));
        }

        #[test]
        fn phantom_path_records_no_hit_and_stays_fused() {
            let (dir, index) = seeded_repo();
            let decision = route("what about missing.rs then", &index, dir.path());
            assert_eq!(decision.plan, Plan::Fused);
            assert!(decision.signals.contains(&"path-token-no-hit".to_string()));
        }
    }
}
