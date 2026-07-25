use std::fs;
use std::path::Path;
use std::process::Command;

use lineage_core::{
    AgentKind, Artifact, ArtifactKind, Confidence, Conversation, LineObject, LineageId, Role, Turn,
};
use lineage_git::{open_repo, persist_conversation, write_line_object, write_note_for_commit};
use lineage_search::LineageIndex;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init"]);
    git(dir.path(), &["config", "user.email", "t@t.dev"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    dir
}

fn commit_file(dir: &Path, path: &str, contents: &str, msg: &str) -> String {
    fs::write(dir.path_join(path), contents).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

trait PathJoin {
    fn path_join(&self, p: &str) -> std::path::PathBuf;
}
impl PathJoin for Path {
    fn path_join(&self, p: &str) -> std::path::PathBuf {
        self.join(p)
    }
}

/// A conversation with one FileEdit turn, so a materialized line object has a
/// real turn to point at. Returns (conversation, turn_id).
fn conv_with_turn(dir: &Path, path: &str, range: [u32; 2]) -> (Conversation, LineageId) {
    let mut conv = Conversation::new(AgentKind::Claude, dir.display().to_string());
    let turn = Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: format!("edited {path}"),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![Artifact {
            kind: ArtifactKind::FileEdit,
            path: path.into(),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: Some(range),
            resolve: None,
        }],
    };
    let turn_id = turn.id.clone();
    conv.turns.push(turn);
    (conv, turn_id)
}

/// Attach a line object (and its note) to a commit for a conversation's turn.
fn attach_line_object(
    repo: &lineage_git::LineageRepo,
    conv: &Conversation,
    turn_id: &LineageId,
    path: &str,
    range: [u32; 2],
    commit_sha: &str,
    confidence: Confidence,
) -> LineObject {
    let obj = LineObject::new(
        path,
        range,
        commit_sha,
        conv.id.clone(),
        turn_id.clone(),
        confidence,
    );
    write_line_object(repo.inner(), &obj).unwrap();
    write_note_for_commit(
        repo.inner(),
        commit_sha,
        std::slice::from_ref(&conv.id),
        std::slice::from_ref(&obj.id),
        None,
    )
    .unwrap();
    obj
}

fn open_index(dir: &Path) -> LineageIndex {
    LineageIndex::open(dir.join(".git").join("lineage").join("index.db")).unwrap()
}

/// A file edited across three commits, each linked to a turn, chains through
/// every commit down to the root boundary — and the query is one anchor plus
/// indexed reads (`line_history` takes no repo).
#[test]
fn walk_resolves_full_chain_to_boundary() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();

    let c1 = commit_file(dir.path(), "f.txt", "alpha\nbeta\ngamma\n", "c1");
    let c2 = commit_file(dir.path(), "f.txt", "alpha\nbeta EDIT\ngamma\n", "c2");
    let c3 = commit_file(dir.path(), "f.txt", "alpha\nbeta EDIT2\ngamma\n", "c3");

    let (conv1, t1) = conv_with_turn(dir.path(), "f.txt", [2, 2]);
    let (conv2, t2) = conv_with_turn(dir.path(), "f.txt", [2, 2]);
    let (conv3, t3) = conv_with_turn(dir.path(), "f.txt", [2, 2]);
    persist_conversation(repo.inner(), &conv1).unwrap();
    persist_conversation(repo.inner(), &conv2).unwrap();
    persist_conversation(repo.inner(), &conv3).unwrap();
    attach_line_object(&repo, &conv1, &t1, "f.txt", [2, 2], &c1, Confidence::Exact);
    attach_line_object(&repo, &conv2, &t2, "f.txt", [2, 2], &c2, Confidence::Exact);
    attach_line_object(&repo, &conv3, &t3, "f.txt", [2, 2], &c3, Confidence::Exact);

    let index = open_index(dir.path());
    let mirrored = index
        .populate_line_tables(repo.inner(), &mut |_, _| {})
        .unwrap();
    assert_eq!(mirrored, 3);

    // Anchor line 2 at c3 (its HEAD commit); the walk is pure index after that.
    let hops = index.line_history("f.txt", 2, &c3).unwrap();
    let commits: Vec<&str> = hops.iter().map(|h| h.commit_sha.as_str()).collect();
    assert_eq!(commits, vec![c3.as_str(), c2.as_str(), c1.as_str()]);

    // Every hop resolves to its turn.
    assert_eq!(hops[0].turn_id.as_deref(), Some(t3.as_str()));
    assert_eq!(hops[1].turn_id.as_deref(), Some(t2.as_str()));
    assert_eq!(hops[2].turn_id.as_deref(), Some(t1.as_str()));

    // The root commit is a boundary (no parent to follow).
    assert_eq!(hops[2].hop_kind, "boundary");
    assert!(hops[0].hop_kind == "resolved" || hops[0].hop_kind == "boundary");
}

/// A commit on the line's ancestry that carries no lineage note is a dark hop:
/// the chain continues through it (an edge exists), attributing nothing.
#[test]
fn dark_hop_continues_the_chain() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();

    let c1 = commit_file(dir.path(), "f.txt", "one\ntwo\nthree\n", "c1");
    // c2 edits line 2 but is never linked — a dark commit in the middle.
    let _c2 = commit_file(dir.path(), "f.txt", "one\ntwo DARK\nthree\n", "c2");
    let c3 = commit_file(dir.path(), "f.txt", "one\ntwo DARK EDIT\nthree\n", "c3");

    let (conv1, t1) = conv_with_turn(dir.path(), "f.txt", [2, 2]);
    let (conv3, t3) = conv_with_turn(dir.path(), "f.txt", [2, 2]);
    persist_conversation(repo.inner(), &conv1).unwrap();
    persist_conversation(repo.inner(), &conv3).unwrap();
    attach_line_object(&repo, &conv1, &t1, "f.txt", [2, 2], &c1, Confidence::Exact);
    attach_line_object(&repo, &conv3, &t3, "f.txt", [2, 2], &c3, Confidence::Exact);

    let index = open_index(dir.path());
    index
        .populate_line_tables(repo.inner(), &mut |_, _| {})
        .unwrap();

    let hops = index.line_history("f.txt", 2, &c3).unwrap();
    // c3 resolved, c2 dark (no note), c1 resolved+boundary — the walk did not
    // strand at the dark hop.
    assert_eq!(hops[0].turn_id.as_deref(), Some(t3.as_str()));
    let dark = hops.iter().find(|h| h.hop_kind == "dark_no_note");
    assert!(dark.is_some(), "expected a dark_no_note hop, got {hops:?}");
    assert!(dark.unwrap().turn_id.is_none());
    // The chain reaches c1's turn past the dark hop.
    assert!(hops
        .iter()
        .any(|h| h.turn_id.as_deref() == Some(t1.as_str())));
}

/// A commit whose note covers a *different* region than the queried line is
/// `dark_no_match` (note present, no covering object) — distinct from no note.
#[test]
fn note_without_covering_object_is_dark_no_match() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();

    let c1 = commit_file(dir.path(), "f.txt", "one\ntwo\nthree\nfour\n", "c1");
    // c2 edits BOTH line 2 and line 3, but its note only covers line 3. Line 2
    // passing through c2 is then dark_no_match: note present, line 2 uncovered.
    let c2 = commit_file(
        dir.path(),
        "f.txt",
        "one\ntwo EDIT\nthree EDIT\nfour\n",
        "c2",
    );
    let c3 = commit_file(
        dir.path(),
        "f.txt",
        "one\ntwo EDIT2\nthree EDIT\nfour\n",
        "c3",
    );

    let (conv1, t1) = conv_with_turn(dir.path(), "f.txt", [2, 2]);
    let (conv2, t2) = conv_with_turn(dir.path(), "f.txt", [3, 3]);
    let (conv3, t3) = conv_with_turn(dir.path(), "f.txt", [2, 2]);
    persist_conversation(repo.inner(), &conv1).unwrap();
    persist_conversation(repo.inner(), &conv2).unwrap();
    persist_conversation(repo.inner(), &conv3).unwrap();
    attach_line_object(&repo, &conv1, &t1, "f.txt", [2, 2], &c1, Confidence::Exact);
    attach_line_object(&repo, &conv2, &t2, "f.txt", [3, 3], &c2, Confidence::Exact);
    attach_line_object(&repo, &conv3, &t3, "f.txt", [2, 2], &c3, Confidence::Exact);

    let index = open_index(dir.path());
    index
        .populate_line_tables(repo.inner(), &mut |_, _| {})
        .unwrap();

    let hops = index.line_history("f.txt", 2, &c3).unwrap();
    // Line 2's path through c2 is dark_no_match (c2 has a note, but it covers
    // line 3, not line 2).
    let has_no_match = hops.iter().any(|h| h.hop_kind == "dark_no_match");
    assert!(has_no_match, "expected dark_no_match hop, got {hops:?}");
}

/// Two line objects covering the same region (nested attributions) both mirror,
/// and containment picks the narrowest for the hop's turn.
#[test]
fn multiple_objects_on_one_region_narrowest_wins() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();

    let c1 = commit_file(dir.path(), "f.txt", "a\nb\nc\nd\ne\n", "c1");

    let (broad_conv, broad_t) = conv_with_turn(dir.path(), "f.txt", [1, 5]);
    let (narrow_conv, narrow_t) = conv_with_turn(dir.path(), "f.txt", [3, 3]);
    persist_conversation(repo.inner(), &broad_conv).unwrap();
    persist_conversation(repo.inner(), &narrow_conv).unwrap();
    attach_line_object(
        &repo,
        &broad_conv,
        &broad_t,
        "f.txt",
        [1, 5],
        &c1,
        Confidence::Heuristic,
    );
    attach_line_object(
        &repo,
        &narrow_conv,
        &narrow_t,
        "f.txt",
        [3, 3],
        &c1,
        Confidence::Exact,
    );

    let index = open_index(dir.path());
    index
        .populate_line_tables(repo.inner(), &mut |_, _| {})
        .unwrap();

    // Both objects mirrored under the file.
    let objs = index.line_objects_for_file("f.txt").unwrap();
    assert_eq!(objs.len(), 2);

    // Line 3 resolves to the narrower [3,3] turn, not the broad [1,5] one.
    let hops = index.line_history("f.txt", 3, &c1).unwrap();
    assert_eq!(hops[0].turn_id.as_deref(), Some(narrow_t.as_str()));
    // Line 1 (only the broad object covers it) resolves to the broad turn.
    let hops1 = index.line_history("f.txt", 1, &c1).unwrap();
    assert_eq!(hops1[0].turn_id.as_deref(), Some(broad_t.as_str()));
}

/// The aggregation query orders a file's line objects by commit time, no
/// walking — the "why so bloated" case `committed_at` earns its column.
#[test]
fn aggregation_by_committed_at_orders_newest_first() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();

    let c1 = commit_file(dir.path(), "f.txt", "a\nb\n", "c1");
    let c2 = commit_file(dir.path(), "f.txt", "a\nb EDIT\n", "c2");

    let (conv1, t1) = conv_with_turn(dir.path(), "f.txt", [1, 1]);
    let (conv2, t2) = conv_with_turn(dir.path(), "f.txt", [2, 2]);
    persist_conversation(repo.inner(), &conv1).unwrap();
    persist_conversation(repo.inner(), &conv2).unwrap();
    attach_line_object(&repo, &conv1, &t1, "f.txt", [1, 1], &c1, Confidence::Exact);
    attach_line_object(&repo, &conv2, &t2, "f.txt", [2, 2], &c2, Confidence::Exact);

    let index = open_index(dir.path());
    index
        .populate_line_tables(repo.inner(), &mut |_, _| {})
        .unwrap();

    let objs = index.line_objects_for_file("f.txt").unwrap();
    assert_eq!(objs.len(), 2);
    // Newest first: c2 (turn t2) before c1 (turn t1).
    assert_eq!(objs[0].turn_id, t2.as_str());
    assert_eq!(objs[1].turn_id, t1.as_str());
    assert!(objs[0].committed_at >= objs[1].committed_at);
}

/// The turn → line object direction: `idx_line_objects_turn` read the way
/// nothing read it before, which is what makes the graph two-way.
#[test]
fn line_objects_for_turn_walks_from_a_turn_to_the_code_it_produced() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();

    let c1 = commit_file(dir.path(), "f.txt", "a\nb\n", "c1");
    let c2 = commit_file(dir.path(), "g.txt", "c\n", "c2");

    let (conv, t) = conv_with_turn(dir.path(), "f.txt", [1, 1]);
    let (other_conv, other_t) = conv_with_turn(dir.path(), "g.txt", [1, 1]);
    persist_conversation(repo.inner(), &conv).unwrap();
    persist_conversation(repo.inner(), &other_conv).unwrap();
    // One turn wrote two files across two commits; another turn wrote one.
    attach_line_object(&repo, &conv, &t, "f.txt", [1, 1], &c1, Confidence::Exact);
    attach_line_object(&repo, &conv, &t, "g.txt", [1, 1], &c2, Confidence::Exact);
    attach_line_object(
        &repo,
        &other_conv,
        &other_t,
        "g.txt",
        [1, 1],
        &c2,
        Confidence::Heuristic,
    );

    let index = open_index(dir.path());
    index
        .populate_line_tables(repo.inner(), &mut |_, _| {})
        .unwrap();

    let objs = index.line_objects_for_turn(t.as_str(), 10).unwrap();
    assert_eq!(objs.len(), 2, "only this turn's objects");
    assert!(objs.iter().all(|o| o.turn_id == t.as_str()));
    let mut files: Vec<&str> = objs.iter().map(|o| o.file_path.as_str()).collect();
    files.sort();
    assert_eq!(files, vec!["f.txt", "g.txt"]);
    assert_eq!(index.line_objects_for_turn(t.as_str(), 1).unwrap().len(), 1);
    assert!(index
        .line_objects_for_turn("no-such-turn", 10)
        .unwrap()
        .is_empty());
}

/// Full recompute wipes and rebuilds both tables: a stale object dropped from
/// the refs does not survive a repopulate.
#[test]
fn populate_is_idempotent_and_drops_stale() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let c1 = commit_file(dir.path(), "f.txt", "x\ny\n", "c1");

    let (conv, t) = conv_with_turn(dir.path(), "f.txt", [1, 1]);
    persist_conversation(repo.inner(), &conv).unwrap();
    let obj = attach_line_object(&repo, &conv, &t, "f.txt", [1, 1], &c1, Confidence::Exact);

    let index = open_index(dir.path());
    index
        .populate_line_tables(repo.inner(), &mut |_, _| {})
        .unwrap();
    assert_eq!(index.line_objects_for_file("f.txt").unwrap().len(), 1);

    // Repopulate with no change — count stays 1 (not doubled).
    index
        .populate_line_tables(repo.inner(), &mut |_, _| {})
        .unwrap();
    assert_eq!(index.line_objects_for_file("f.txt").unwrap().len(), 1);

    // Remove the object ref; a repopulate drops the mirrored row.
    let ref_name = format!("refs/lineage/lines/{}", obj.id.as_str());
    git(dir.path(), &["update-ref", "-d", &ref_name]);
    index
        .populate_line_tables(repo.inner(), &mut |_, _| {})
        .unwrap();
    assert!(index.line_objects_for_file("f.txt").unwrap().is_empty());
}
