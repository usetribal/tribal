use std::path::Path;

use git2::{BlameOptions, Repository};
use lineage_core::LineageError;

/// One blame-ancestry edge: a (commit, file, region) child position and the
/// parent position the region came from. `parent` is `None` only when the child
/// commit introduced the region (a boundary — no `previous` to follow). Regions
/// are blame-hunk grain: a hunk is a contiguous run of lines sharing one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AncestryHop {
    pub commit_sha: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub parent: Option<AncestryParent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AncestryParent {
    pub commit_sha: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// One blame hunk over a queried region: the commit that last touched a
/// contiguous run of lines, that run's position within that commit, and the run
/// at the queried revision.
struct RegionHunk {
    commit_sha: String,
    file_path: String,
    orig_start: u32,
    orig_end: u32,
}

fn blame_options(start: u32, end: u32) -> BlameOptions {
    let mut opts = BlameOptions::new();
    opts.min_line(start as usize);
    opts.max_line(end as usize);
    // Mirror the prototype's `-w -C` spine: whitespace-insensitive, same-file
    // copy tracking, so rename/move history is followed the way chain.sh does.
    opts.track_copies_same_file(true);
    opts.ignore_whitespace(true);
    opts
}

/// Blame `file_path` as it stands at `rev` over `[start, end]`, returning one
/// [`RegionHunk`] per distinct blame hunk touching the region. A path absent at
/// `rev` (pre-rename, pre-creation) yields an empty vec — the walk terminates
/// rather than erroring.
fn blame_region(
    repo: &Repository,
    rev: &str,
    file_path: &str,
    start: u32,
    end: u32,
) -> Result<Vec<RegionHunk>, LineageError> {
    let oid = git2::Oid::from_str(rev).map_err(|e| LineageError::Other(e.to_string()))?;
    let mut opts = blame_options(start, end);
    opts.newest_commit(oid);

    let blame = match repo.blame_file(Path::new(file_path), Some(&mut opts)) {
        Ok(b) => b,
        Err(_) => return Ok(Vec::new()),
    };

    let mut hunks = Vec::new();
    for line in start..=end {
        let Some(hunk) = blame.get_line(line as usize) else {
            continue;
        };
        let commit_sha = hunk.final_commit_id().to_string();
        // orig_start_line is the hunk's first line within its commit; the line's
        // offset inside the hunk maps the queried line to its orig position.
        let final_start = hunk.final_start_line() as u32;
        let orig_start = hunk.orig_start_line() as u32;
        let orig_line = orig_start + (line - final_start);
        let hunk_path = hunk
            .path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| file_path.to_string());

        // Coalesce consecutive lines of the same hunk into one region.
        match hunks.last_mut() {
            Some(RegionHunk {
                commit_sha: last_sha,
                orig_end,
                ..
            }) if *last_sha == commit_sha && *orig_end + 1 == orig_line => {
                *orig_end = orig_line;
            }
            _ => hunks.push(RegionHunk {
                commit_sha,
                file_path: hunk_path,
                orig_start: orig_line,
                orig_end: orig_line,
            }),
        }
    }
    Ok(hunks)
}

/// The commit and orig-line a single line resolves to at `rev`, blamed with the
/// same `-w -C` options the ancestry walk uses. This is the one live blame a
/// chain query is allowed: it anchors HEAD to the first commit, and the indexed
/// tables carry the walk from there. `None` if the line has no blame at `rev`.
pub fn resolve_anchor(
    repo: &Repository,
    rev: &str,
    file_path: &str,
    line: u32,
) -> Result<Option<(String, u32)>, LineageError> {
    let hunks = blame_region(repo, rev, file_path, line, line)?;
    Ok(hunks
        .into_iter()
        .next()
        .map(|h| (h.commit_sha, h.orig_start)))
}

/// Unix commit time (committer, seconds) for `commit_sha`, or `None` if the
/// commit no longer exists — a line object pointing at unreachable history.
pub fn commit_time(repo: &Repository, commit_sha: &str) -> Result<Option<i64>, LineageError> {
    let oid = match git2::Oid::from_str(commit_sha) {
        Ok(o) => o,
        Err(_) => return Ok(None),
    };
    match repo.find_commit(oid) {
        Ok(commit) => Ok(Some(commit.time().seconds())),
        Err(_) => Ok(None),
    }
}

/// The first-parent sha of `commit_sha`, or `None` at a root commit.
fn first_parent_sha(repo: &Repository, commit_sha: &str) -> Result<Option<String>, LineageError> {
    let oid = git2::Oid::from_str(commit_sha).map_err(|e| LineageError::Other(e.to_string()))?;
    let commit = repo
        .find_commit(oid)
        .map_err(|e| LineageError::Other(e.to_string()))?;
    Ok(commit.parents().next().map(|p| p.id().to_string()))
}

/// The parent region a hunk at `(commit, file, orig_start..orig_end)` came
/// from: blame the first parent over that orig range. `None` if the commit
/// introduced the region (parent has no blame there — a boundary) or is a root.
fn parent_hunk(
    repo: &Repository,
    hunk: &RegionHunk,
) -> Result<Option<AncestryParent>, LineageError> {
    let Some(parent_sha) = first_parent_sha(repo, &hunk.commit_sha)? else {
        return Ok(None);
    };
    let parents = blame_region(
        repo,
        &parent_sha,
        &hunk.file_path,
        hunk.orig_start,
        hunk.orig_end,
    )?;
    // Span every parent hunk into one edge: the child region's ancestry is
    // whatever the parent blame resolved it to, and the walk continues from the
    // union. Sub-range divergence within the parent is recovered on the next
    // hop's own blame, keyed by the recursion below.
    let Some(first) = parents.first() else {
        return Ok(None);
    };
    let last = parents.last().unwrap_or(first);
    Ok(Some(AncestryParent {
        commit_sha: first.commit_sha.clone(),
        file_path: first.file_path.clone(),
        start_line: first.orig_start,
        end_line: last.orig_end,
    }))
}

/// Walk a region's blame ancestry backward from `(rev, file_path, [start,
/// end])`, emitting hunk-grain [`AncestryHop`] edges until every branch reaches
/// a boundary or `max_hops`. Each hop blames the region once and recurses on
/// each distinct parent hunk, so a region that splits across commits diverges
/// into separate edges (sub-range divergence) while a single-commit region
/// stays one edge.
pub fn walk_line_ancestry(
    repo: &Repository,
    rev: &str,
    file_path: &str,
    start_line: u32,
    end_line: u32,
    max_hops: usize,
) -> Result<Vec<AncestryHop>, LineageError> {
    let mut seen = std::collections::HashSet::new();
    walk_line_ancestry_shared(
        repo, rev, file_path, start_line, end_line, max_hops, &mut seen,
    )
}

/// [`walk_line_ancestry`] with a caller-owned `seen` set of already-blamed
/// `(commit, orig_start, orig_end)` child positions. Population walks thousands
/// of overlapping regions whose ancestries share commits; a shared set makes
/// each blame position pay once across the whole pass instead of once per seed
/// — the difference between a minutes-long rebuild and an hour-long one.
#[allow(clippy::too_many_arguments)]
pub fn walk_line_ancestry_shared(
    repo: &Repository,
    rev: &str,
    file_path: &str,
    start_line: u32,
    end_line: u32,
    max_hops: usize,
    seen: &mut std::collections::HashSet<(String, u32, u32)>,
) -> Result<Vec<AncestryHop>, LineageError> {
    let mut hops = Vec::new();
    let mut frontier = vec![(rev.to_string(), file_path.to_string(), start_line, end_line)];

    for _ in 0..max_hops {
        if frontier.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for (rev, file, start, end) in frontier {
            for hunk in blame_region(repo, &rev, &file, start, end)? {
                // A child position already blamed (this seed or an earlier one)
                // has its edges recorded — stop rather than re-blame its parents.
                let key = (hunk.commit_sha.clone(), hunk.orig_start, hunk.orig_end);
                if !seen.insert(key) {
                    continue;
                }
                let parent = parent_hunk(repo, &hunk)?;
                hops.push(AncestryHop {
                    commit_sha: hunk.commit_sha.clone(),
                    file_path: hunk.file_path.clone(),
                    start_line: hunk.orig_start,
                    end_line: hunk.orig_end,
                    parent: parent.clone(),
                });
                if let Some(parent) = parent {
                    next.push((
                        parent.commit_sha,
                        parent.file_path,
                        parent.start_line,
                        parent.end_line,
                    ));
                }
            }
        }
        frontier = next;
    }

    Ok(hops)
}
