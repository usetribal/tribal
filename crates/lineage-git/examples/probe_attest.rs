use std::collections::{BTreeMap, BTreeSet, HashSet};
use git2::Repository;
use lineage_core::ArtifactKind;

fn main() {
    let repo = Repository::open("/home/xifong/src/lineage-platform").unwrap();
    let workdir = repo.workdir().unwrap().to_string_lossy().trim_end_matches('/').to_string();
    let tracked: HashSet<String> = std::process::Command::new("git")
        .args(["ls-files"]).current_dir("/home/xifong/src/lineage-platform")
        .output().unwrap().stdout.split(|b| *b == b'\n')
        .map(|s| String::from_utf8_lossy(s).to_string()).filter(|s| !s.is_empty()).collect();

    let conversations: Vec<_> = lineage_git::list_session_ids(&repo).unwrap().into_iter()
        .filter_map(|id| lineage_git::read_conversation_stored(&repo, &id).unwrap())
        .collect();

    // (a) every worktree name any session recorded as its own workspace_root
    let mut attested: BTreeSet<String> = BTreeSet::new();
    for c in &conversations {
        let ws = c.workspace_root.replace('\\', "/");
        let rel = ws.strip_prefix(&workdir).map(|r| r.trim_start_matches('/')).unwrap_or("");
        if let Some(rest) = rel.strip_prefix(".claude/worktrees/") {
            if let Some(name) = rest.split('/').next() {
                if !name.is_empty() { attested.insert(name.to_string()); }
            }
        }
    }
    // registered with git today
    let registered: HashSet<String> = repo.worktrees()
        .map(|ns| ns.iter().flatten().map(|n| n.to_string()).collect()).unwrap_or_default();

    // walk artifacts carrying a .claude/worktrees/<name>/ prefix
    let mut by_name: BTreeMap<String, (usize, BTreeSet<String>)> = BTreeMap::new();
    let mut unstripped_tracked = 0usize;
    let mut remainder_untracked = 0usize;
    for c in &conversations {
        for t in &c.turns {
            for a in &t.artifacts {
                if !matches!(a.kind, ArtifactKind::FileEdit | ArtifactKind::Diff) { continue }
                let p = a.path.trim_start_matches("./").replace('\\', "/");
                let rel = p.strip_prefix(&workdir).map(|r| r.trim_start_matches('/').to_string()).unwrap_or(p);
                let Some(rest) = rel.strip_prefix(".claude/worktrees/") else { continue };
                let Some((name, tail)) = rest.split_once('/') else { continue };
                if tail.is_empty() { continue }
                if tracked.contains(&rel) { unstripped_tracked += 1; continue }
                if !tracked.contains(tail) { remainder_untracked += 1; continue }
                let e = by_name.entry(name.to_string()).or_insert((0, BTreeSet::new()));
                e.0 += 1;
                e.1.insert(tail.to_string());
            }
        }
    }

    println!("worktree names attested by some session's workspace_root: {}", attested.len());
    println!("worktree names registered with git today:                 {}\n", registered.len());
    println!("{:<42} {:>6} {:>7}  {}", "prefix name", "arts", "files", "evidence");
    let mut tot_a = (0usize, BTreeSet::new());
    let mut tot_c = (0usize, BTreeSet::new());
    for (name, (n, files)) in &by_name {
        let ev = if registered.contains(name) { "registered" }
                 else if attested.contains(name) { "ATTESTED (a)" }
                 else { "containment only (c)" };
        println!("{name:<42} {n:>6} {:>7}  {ev}", files.len());
        if registered.contains(name) || attested.contains(name) {
            tot_a.0 += n; tot_a.1.extend(files.iter().cloned());
        }
        tot_c.0 += n; tot_c.1.extend(files.iter().cloned());
    }
    println!("\n--- totals ---");
    println!("option (a)+(c) strict: {} artifacts, {} distinct files", tot_a.0, tot_a.1.len());
    println!("option (c) alone:      {} artifacts, {} distinct files", tot_c.0, tot_c.1.len());
    println!("\nfalse-positive guards:");
    println!("  unstripped path itself tracked (would be wrong to strip): {unstripped_tracked}");
    println!("  remainder not a tracked file (rejected by containment):   {remainder_untracked}");
    std::fs::write("/tmp/claude-1000/-home-xifong-src-lineage-platform/9dc5129e-48b1-41f9-b864-b34e8ecb88ca/scratchpad/strict.txt",
        tot_a.1.iter().cloned().collect::<Vec<_>>().join("\n")).unwrap();
    std::fs::write("/tmp/claude-1000/-home-xifong-src-lineage-platform/9dc5129e-48b1-41f9-b864-b34e8ecb88ca/scratchpad/loose.txt",
        tot_c.1.iter().cloned().collect::<Vec<_>>().join("\n")).unwrap();
}
