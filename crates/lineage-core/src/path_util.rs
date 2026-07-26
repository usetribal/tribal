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
}
