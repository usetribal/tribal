use std::path::{Path, PathBuf};

/// Normalize an agent tool path to a repo-relative POSIX path for git comparisons.
///
/// Strips `./`, converts backslashes, and removes `workspace_root` when the path
/// is absolute under that root (including macOS `/var` vs `/private/var` aliases).
pub fn normalize_repo_path(path: &str, workspace_root: Option<&Path>) -> String {
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

/// True when two paths refer to the same repo-relative file after normalization.
pub fn paths_match_repo_file(
    artifact_path: &str,
    repo_relative_path: &str,
    workspace_root: Option<&Path>,
) -> bool {
    let left = normalize_repo_path(artifact_path, workspace_root);
    let right = normalize_repo_path(repo_relative_path, workspace_root);
    left == right
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn strips_leading_dot_slash() {
        assert_eq!(normalize_repo_path("./src/main.rs", None), "src/main.rs");
    }

    #[test]
    fn leaves_relative_paths_unchanged() {
        assert_eq!(
            normalize_repo_path("docs/readme.md", None),
            "docs/readme.md"
        );
    }

    #[test]
    fn strips_absolute_path_against_workspace() {
        let root = Path::new("/Users/dev/my-project");
        assert_eq!(
            normalize_repo_path("/Users/dev/my-project/src/auth.rs", Some(root)),
            "src/auth.rs"
        );
    }

    #[test]
    fn strips_macos_private_var_alias() {
        let root = Path::new("/var/folders/T/tmp-repo");
        assert_eq!(
            normalize_repo_path("/private/var/folders/T/tmp-repo/src/auth.rs", Some(root)),
            "src/auth.rs"
        );
    }

    #[test]
    fn paths_match_after_normalization() {
        let root = Path::new("/Users/dev/proj");
        assert!(paths_match_repo_file(
            "/Users/dev/proj/src/a.rs",
            "src/a.rs",
            Some(root)
        ));
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
}
