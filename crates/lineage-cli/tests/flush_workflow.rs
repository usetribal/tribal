//! The flush that runs before a session selector opens.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use lineage_cli::{commands, flush};
use lineage_git::{list_session_ids, open_repo};
use lineage_search::LineageIndex;

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.email", "test@test.dev"],
        vec!["config", "user.name", "Test"],
    ] {
        Command::new("git")
            .args(&args)
            .current_dir(dir.path())
            .output()
            .unwrap();
    }
    fs::write(dir.path().join("src.txt"), "hello\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn install_cursor_fixture(dir: &Path) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/cursor-history/.cursor");
    copy_dir_all(&fixture, &dir.join(".cursor")).unwrap();
}

fn flush(dir: &Path) -> flush::FlushReport {
    flush::flush_sessions(dir, &mut |_, _| {}).unwrap()
}

#[test]
fn a_session_on_disk_is_in_refs_and_searchable_after_a_flush() {
    let dir = init_repo();
    install_cursor_fixture(dir.path());
    commands::init_config(dir.path()).unwrap();

    let repo = open_repo(dir.path()).unwrap();
    assert!(list_session_ids(repo.inner()).unwrap().is_empty());

    let report = flush(dir.path());
    assert!(report.imported > 0, "nothing was imported");

    let ids = list_session_ids(repo.inner()).unwrap();
    assert_eq!(ids.len(), report.imported);

    // Asserted through search, not by inspecting the index: content search is
    // what the selector actually uses, so a flush that filled refs but not the
    // index would leave the new session unfindable by query.
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db")).unwrap();
    let hits = index.search("the", 50).unwrap();
    assert!(
        !hits.is_empty(),
        "a flushed session should be findable by content search"
    );
    let known: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    assert!(hits.iter().all(|hit| known.contains(&hit.session_id)));
}

#[test]
fn a_second_flush_reimports_nothing() {
    let dir = init_repo();
    install_cursor_fixture(dir.path());
    commands::init_config(dir.path()).unwrap();

    let first = flush(dir.path());
    assert!(first.imported > 0);

    // Nothing on disk changed, so the mtime stamps must make this a no-op.
    let second = flush(dir.path());
    assert_eq!(second.imported, 0);
    assert_eq!(second.skipped, first.imported);
}

#[test]
fn a_flush_reports_progress_against_a_total() {
    let dir = init_repo();
    install_cursor_fixture(dir.path());
    commands::init_config(dir.path()).unwrap();

    let mut seen: Vec<(usize, usize)> = Vec::new();
    flush::flush_sessions(dir.path(), &mut |done, total| seen.push((done, total))).unwrap();

    assert!(!seen.is_empty(), "progress was never reported");
    let (_, total) = seen[0];
    assert!(seen.iter().all(|&(done, t)| done <= t && t == total));
}

#[test]
fn a_flush_on_a_repo_with_no_sessions_is_harmless() {
    let dir = init_repo();
    commands::init_config(dir.path()).unwrap();

    let report = flush(dir.path());
    assert_eq!(report.imported, 0);
    assert_eq!(report.failed, 0);
}
