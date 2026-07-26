use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Normalize a path with no knowledge of the repository it belongs to.
///
/// Strips `./`, converts backslashes, and removes `workspace_root` when the path
/// is absolute under that root (including macOS `/var` vs `/private/var` aliases).
///
/// **Repo-unaware**: this cannot recognise a path recorded through a linked
/// worktree, and will return such a path unchanged — which matches no file in
/// any commit diff and silently drops the edit from the provenance graph. Use it
/// only where there is genuinely no repository in scope (parsing a `file:line`
/// argument, comparing two already-normalized stored paths). Anywhere a
/// `Repository` is reachable, build a [`RepoPaths`] and call
/// [`RepoPaths::normalize`] instead.
pub fn normalize_repo_path_unscoped(path: &str, workspace_root: Option<&Path>) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut normalized = trimmed.replace('\\', "/");
    while normalized.starts_with("./") {
        normalized = normalized.trim_start_matches("./").to_string();
    }

    if Path::new(&normalized).is_relative() {
        return normalized;
    }

    let Some(root) = workspace_root else {
        return normalized;
    };

    if let Some(rel) = strip_workspace_prefix(&normalized, root) {
        return rel;
    }

    normalized
}

fn strip_workspace_prefix(path: &str, workspace_root: &Path) -> Option<String> {
    let path_norm = path.replace('\\', "/");
    for root in workspace_root_candidates(workspace_root) {
        let root_norm = root.replace('\\', "/");
        let prefix = if root_norm.ends_with('/') {
            root_norm.clone()
        } else {
            format!("{root_norm}/")
        };
        if path_norm == root_norm {
            return Some(String::new());
        }
        if let Some(rest) = path_norm.strip_prefix(&prefix) {
            return Some(rest.trim_start_matches('/').to_string());
        }
    }
    None
}

fn workspace_root_candidates(workspace_root: &Path) -> Vec<String> {
    let mut roots = Vec::new();
    let base = workspace_root.display().to_string().replace('\\', "/");
    roots.push(base.clone());

    if let Ok(canon) = workspace_root.canonicalize() {
        let c = canon.display().to_string().replace('\\', "/");
        if !roots.contains(&c) {
            roots.push(c);
        }
    }

    if base.starts_with("/var/") {
        let private = format!("/private{base}");
        if !roots.contains(&private) {
            roots.push(private);
        }
    } else if base.starts_with("/private/var/") {
        let without = base.strip_prefix("/private").unwrap_or(&base).to_string();
        if !roots.contains(&without) {
            roots.push(without);
        }
    }

    roots
}

/// Resolve workspace root for materialization: conversation field, else git workdir.
pub fn workspace_root_for(conversation_root: &str, git_workdir: Option<&Path>) -> PathBuf {
    let conv = Path::new(conversation_root);
    if conv.is_absolute() {
        return conv.to_path_buf();
    }
    if let Some(workdir) = git_workdir {
        return workdir.to_path_buf();
    }
    conv.to_path_buf()
}

/// Everything path normalization needs to know about one repository: where the
/// session recorded its paths, and where that repository's linked worktrees sit
/// relative to the main workdir.
///
/// Built once per operation and passed down, so a caller cannot accidentally
/// normalize without worktree knowledge. That mistake is silent — a path
/// recorded through a worktree matches no file in any commit diff, so the edit
/// is dropped from the provenance graph with no error anywhere — which is why
/// the context is a parameter rather than a convention.
///
/// `worktree_prefixes` are repo-relative directory prefixes (`".claude/worktrees/x/"`),
/// computed by the caller that owns repository layout. This type does pure
/// string stripping and never inspects the filesystem, so it stays free of any
/// notion of what a worktree is or where one may live.
#[derive(Debug, Clone, Default)]
pub struct RepoPaths {
    workspace_root: Option<PathBuf>,
    worktree_prefixes: Vec<String>,
}

impl RepoPaths {
    /// Each prefix must be repo-relative and directory-like; a trailing slash is
    /// added when missing so `.claude/worktrees/ab` never matches a path under
    /// a sibling `.claude/worktrees/abc`.
    pub fn new(workspace_root: Option<&Path>, worktree_prefixes: Vec<String>) -> Self {
        Self {
            workspace_root: workspace_root.map(Path::to_path_buf),
            worktree_prefixes: worktree_prefixes
                .into_iter()
                .filter(|prefix| !prefix.is_empty())
                .map(|prefix| match prefix.ends_with('/') {
                    true => prefix,
                    false => format!("{prefix}/"),
                })
                .collect(),
        }
    }

    /// A context for a repository with no linked worktrees.
    pub fn rooted_at(workspace_root: Option<&Path>) -> Self {
        Self::new(workspace_root, Vec::new())
    }

    /// The same repository seen from a different session's workspace root.
    ///
    /// Worktree prefixes are a property of the repository, not of the session,
    /// so a caller sweeping many conversations resolves them once and rebases
    /// per session rather than re-reading git's registry each time.
    pub fn with_workspace_root(&self, workspace_root: &Path) -> Self {
        Self {
            workspace_root: Some(workspace_root.to_path_buf()),
            worktree_prefixes: self.worktree_prefixes.clone(),
        }
    }

    /// Normalize an artifact path to repo-relative, rebasing paths recorded
    /// through one of this repository's linked worktrees.
    ///
    /// A worktree checks out the same repository at another path, so an edit
    /// made there is an edit to a tracked file. Some sessions record such an
    /// edit relative to the *main* workdir rather than the worktree they ran in
    /// — `.claude/worktrees/feature/AGENTS.md` for what is really `AGENTS.md`.
    /// That path is already relative, so plain normalization leaves it
    /// untouched and every edit the session made is lost.
    pub fn normalize(&self, path: &str) -> String {
        let normalized = normalize_repo_path_unscoped(path, self.workspace_root.as_deref());
        if normalized.is_empty() {
            return normalized;
        }
        for prefix in &self.worktree_prefixes {
            if let Some(rest) = normalized.strip_prefix(prefix.as_str()) {
                return rest.trim_start_matches('/').to_string();
            }
        }
        normalized
    }

    /// True when two paths refer to the same repo-relative file once both are
    /// normalized through this context.
    pub fn paths_match(&self, artifact_path: &str, repo_relative_path: &str) -> bool {
        self.normalize(artifact_path) == self.normalize(repo_relative_path)
    }

    /// Normalize `path`, then — only if the result names nothing in `known` —
    /// try recovering a path recorded through a worktree git no longer knows
    /// about.
    ///
    /// A worktree is usually deleted once its branch merges, and git keeps no
    /// record that it ever existed. Sessions run inside one can record edits
    /// prefixed by its location (`.claude/worktrees/gone/src/auth.rs`), so
    /// [`Self::normalize`] leaves them untouched and every such edit is dropped.
    /// The registry cannot vouch for those prefixes, so two properties of the
    /// stored data stand in for it: the remainder must name something in
    /// `known`, and the path as recorded must not (a repository that genuinely
    /// tracks files under a worktree-shaped directory keeps its own paths).
    ///
    /// `known` is the commit's file set — durable, pushed, and identical on
    /// every machine — which keeps the result deterministic and backfillable.
    /// It is still an inference rather than something git asserts, so a
    /// recovered path reports [`PathOrigin::InferredWorktree`] and callers
    /// weaken the confidence of anything derived from it.
    pub fn resolve_against(&self, path: &str, known: &HashSet<String>) -> (String, PathOrigin) {
        let normalized = self.normalize(path);
        // Empty is nothing to resolve; a hit means the path as recorded is
        // itself tracked, so it means what it says and must not be stripped.
        if normalized.is_empty() || known.contains(&normalized) {
            return (normalized, PathOrigin::Recorded);
        }
        match strip_leading_segments(&normalized, known) {
            Some(recovered) => (recovered, PathOrigin::InferredWorktree),
            None => (normalized, PathOrigin::Recorded),
        }
    }
}

/// How a normalized path was arrived at, so callers can tell a path git vouches
/// for from one recovered by inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathOrigin {
    /// Normalized from what the session recorded, via the workspace root or a
    /// worktree git still has registered.
    Recorded,
    /// Recovered by dropping a leading prefix that looked like a deleted
    /// worktree's location. Provenance derived from this is heuristic.
    InferredWorktree,
}

/// Worktree locations are shallow — a directory or two of nesting
/// (`.claude/worktrees/<name>`, `worktrees/<name>`, `<name>`) — so trying more
/// depth than this buys nothing and costs a lookup per artifact in the
/// materialization loop. It also bounds the false-positive surface: the deeper
/// the strip, the likelier some unrelated suffix coincidentally names a file.
const MAX_WORKTREE_PREFIX_SEGMENTS: usize = 4;

/// The first suffix of `path` that names something in `known`, dropping one
/// leading directory segment at a time. Shallowest strip wins, so a path that
/// resolves after removing one segment is never mistaken for a deeper one.
fn strip_leading_segments(path: &str, known: &HashSet<String>) -> Option<String> {
    let mut rest = path;
    for _ in 0..MAX_WORKTREE_PREFIX_SEGMENTS {
        let (_, tail) = rest.split_once('/')?;
        if tail.is_empty() {
            return None;
        }
        if known.contains(tail) {
            return Some(tail.to_string());
        }
        rest = tail;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn strips_leading_dot_slash() {
        assert_eq!(
            normalize_repo_path_unscoped("./src/main.rs", None),
            "src/main.rs"
        );
    }

    #[test]
    fn leaves_relative_paths_unchanged() {
        assert_eq!(
            normalize_repo_path_unscoped("docs/readme.md", None),
            "docs/readme.md"
        );
    }

    #[test]
    fn strips_absolute_path_against_workspace() {
        let root = Path::new("/Users/dev/my-project");
        assert_eq!(
            normalize_repo_path_unscoped("/Users/dev/my-project/src/auth.rs", Some(root)),
            "src/auth.rs"
        );
    }

    #[test]
    fn strips_macos_private_var_alias() {
        let root = Path::new("/var/folders/T/tmp-repo");
        assert_eq!(
            normalize_repo_path_unscoped("/private/var/folders/T/tmp-repo/src/auth.rs", Some(root)),
            "src/auth.rs"
        );
    }

    #[test]
    fn paths_match_after_normalization() {
        let paths = RepoPaths::rooted_at(Some(Path::new("/Users/dev/proj")));
        assert!(paths.paths_match("/Users/dev/proj/src/a.rs", "src/a.rs"));
    }

    #[test]
    fn workspace_root_for_prefers_absolute_conversation_root() {
        let root = workspace_root_for("/abs/repo", Some(Path::new("/git/workdir")));
        assert_eq!(root, Path::new("/abs/repo"));
    }

    #[test]
    fn workspace_root_for_falls_back_to_git_workdir() {
        let root = workspace_root_for(".", Some(Path::new("/git/workdir")));
        assert_eq!(root, Path::new("/git/workdir"));
    }

    fn with_worktree() -> RepoPaths {
        RepoPaths::new(
            Some(Path::new("/repo")),
            vec![".claude/worktrees/feature".to_string()],
        )
    }

    #[test]
    fn strips_nested_worktree_prefix_from_relative_path() {
        assert_eq!(
            with_worktree().normalize(".claude/worktrees/feature/AGENTS.md"),
            "AGENTS.md"
        );
    }

    #[test]
    fn leaves_lookalike_worktree_path_alone_when_no_prefix_registered() {
        // Only git's registry decides what a worktree is; an unregistered
        // directory of the same shape is an ordinary tracked path.
        let paths = RepoPaths::rooted_at(Some(Path::new("/repo")));
        assert_eq!(
            paths.normalize(".claude/worktrees/feature/AGENTS.md"),
            ".claude/worktrees/feature/AGENTS.md"
        );
    }

    #[test]
    fn leaves_ordinary_repo_paths_untouched() {
        assert_eq!(with_worktree().normalize("src/auth.rs"), "src/auth.rs");
    }

    #[test]
    fn strips_worktree_prefix_from_absolute_path() {
        assert_eq!(
            with_worktree().normalize("/repo/.claude/worktrees/feature/src/auth.rs"),
            "src/auth.rs"
        );
    }

    #[test]
    fn does_not_strip_a_prefix_that_is_only_a_name_fragment() {
        // `feature` must not swallow paths under a sibling `feature-two`.
        assert_eq!(
            with_worktree().normalize(".claude/worktrees/feature-two/AGENTS.md"),
            ".claude/worktrees/feature-two/AGENTS.md"
        );
    }

    #[test]
    fn matches_a_worktree_path_against_its_repo_relative_twin() {
        assert!(with_worktree().paths_match(".claude/worktrees/feature/AGENTS.md", "AGENTS.md"));
    }

    fn known(paths: &[&str]) -> HashSet<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn recovers_a_path_prefixed_by_a_deleted_worktree() {
        let paths = RepoPaths::rooted_at(Some(Path::new("/repo")));
        let (resolved, origin) = paths.resolve_against(
            ".claude/worktrees/gone/src/auth.rs",
            &known(&["src/auth.rs"]),
        );
        assert_eq!(resolved, "src/auth.rs");
        assert_eq!(origin, PathOrigin::InferredWorktree);
    }

    #[test]
    fn leaves_a_path_alone_when_the_remainder_names_nothing() {
        // Containment is the only evidence a deleted worktree existed; without
        // it the prefix is presumed to be a real directory.
        let paths = RepoPaths::rooted_at(Some(Path::new("/repo")));
        let (resolved, origin) = paths.resolve_against(
            ".claude/worktrees/gone/src/auth.rs",
            &known(&["src/other.rs"]),
        );
        assert_eq!(resolved, ".claude/worktrees/gone/src/auth.rs");
        assert_eq!(origin, PathOrigin::Recorded);
    }

    #[test]
    fn leaves_a_path_alone_when_it_is_itself_tracked() {
        // A repository that genuinely tracks files under a worktree-shaped
        // directory keeps its own paths, even though the suffix also resolves.
        let paths = RepoPaths::rooted_at(Some(Path::new("/repo")));
        let (resolved, origin) = paths.resolve_against(
            ".claude/worktrees/gone/src/auth.rs",
            &known(&[".claude/worktrees/gone/src/auth.rs", "src/auth.rs"]),
        );
        assert_eq!(resolved, ".claude/worktrees/gone/src/auth.rs");
        assert_eq!(origin, PathOrigin::Recorded);
    }

    #[test]
    fn registered_worktree_paths_resolve_without_inference() {
        let (resolved, origin) = with_worktree().resolve_against(
            ".claude/worktrees/feature/AGENTS.md",
            &known(&["AGENTS.md"]),
        );
        assert_eq!(resolved, "AGENTS.md");
        assert_eq!(origin, PathOrigin::Recorded);
    }

    #[test]
    fn prefers_the_shallowest_strip_that_resolves() {
        let paths = RepoPaths::rooted_at(Some(Path::new("/repo")));
        let (resolved, _) =
            paths.resolve_against("wt/src/auth.rs", &known(&["src/auth.rs", "auth.rs"]));
        assert_eq!(resolved, "src/auth.rs");
    }

    #[test]
    fn gives_up_beyond_the_segment_bound() {
        let paths = RepoPaths::rooted_at(Some(Path::new("/repo")));
        let deep = "a/b/c/d/e/src/auth.rs";
        let (resolved, origin) = paths.resolve_against(deep, &known(&["src/auth.rs"]));
        assert_eq!(resolved, deep);
        assert_eq!(origin, PathOrigin::Recorded);
    }

    #[test]
    fn resolves_an_ordinary_tracked_path_without_stripping() {
        let paths = RepoPaths::rooted_at(Some(Path::new("/repo")));
        let (resolved, origin) = paths.resolve_against("src/auth.rs", &known(&["src/auth.rs"]));
        assert_eq!(resolved, "src/auth.rs");
        assert_eq!(origin, PathOrigin::Recorded);
    }
}
