use std::fs;
use std::path::PathBuf;

use git2::Repository;
use lineage_core::LineageError;
use lineage_store::LfsStore;

pub const LINEAGE_MEDIA_DIR: &str = ".lineage/media";
pub const GITATTRIBUTES_MARKER: &str = "# lineage-lfs";

pub fn ensure_gitattributes(repo: &Repository) -> Result<(), LineageError> {
    let workdir = repo_workdir(repo)?;
    let path = workdir.join(".gitattributes");
    let entry = format!(
        "{LINEAGE_MEDIA_DIR}/** filter=lfs diff=lfs merge=lfs -text\n{GITATTRIBUTES_MARKER}\n"
    );

    let existing = fs::read_to_string(&path).unwrap_or_default();
    if existing.contains(GITATTRIBUTES_MARKER) {
        return Ok(());
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&entry);
    fs::write(&path, content).map_err(|e| LineageError::Other(e.to_string()))?;
    Ok(())
}

pub fn write_worktree_pointer(
    repo: &Repository,
    oid: &str,
    size: usize,
) -> Result<PathBuf, LineageError> {
    let workdir = repo_workdir(repo)?;
    let rel = worktree_media_path(oid);
    let path = workdir.join(&rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| LineageError::Other(e.to_string()))?;
    }
    let pointer = LfsStore::pointer_text(oid, size);
    fs::write(&path, pointer).map_err(|e| LineageError::Other(e.to_string()))?;
    Ok(path)
}

pub fn worktree_media_path(oid: &str) -> String {
    let oid = oid.trim().strip_prefix("sha256:").unwrap_or(oid);
    format!("{LINEAGE_MEDIA_DIR}/{}/{oid}", &oid[0..2])
}

fn repo_workdir(repo: &Repository) -> Result<PathBuf, LineageError> {
    repo.workdir()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| LineageError::Other("bare repository has no worktree".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_media_path_shards_oid() {
        let path = worktree_media_path("sha256:abcdef0123456789");
        assert!(path.starts_with(LINEAGE_MEDIA_DIR));
        assert!(path.contains("/ab/abcdef0123456789"));
    }
}
