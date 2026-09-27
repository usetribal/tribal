use std::path::{Path, PathBuf};

/// Whether live runs should emit profiling JSON on stderr.
pub fn profiling_enabled() -> bool {
    matches!(
        std::env::var("TRIBAL_TUI_PROFILE").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// Repository for harness tests — see `oss/harness/profiler/README.md`.
pub fn profile_repo_from_env(manifest_dir: &Path) -> PathBuf {
    std::env::var("TRIBAL_TUI_PROFILE_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            manifest_dir
                .ancestors()
                .find(|p| p.join(".git").exists())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        })
}
