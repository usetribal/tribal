//! Provenance coverage behind doctor's `coverage` section (diagnostics-v0):
//! how much of the tracked tree line objects can actually explain.
//!
//! The funnel next door answers "is capture healthy?"; this answers "how much
//! of the codebase can we explain?". Reach (files with any coverage) and depth
//! (coverage within those files) stay separate numbers because they mean
//! different things — a file no captured session ever touched being dark is
//! expected, whereas a touched file with no line objects is a defect.

use std::collections::BTreeMap;

use git2::Repository;
use lineage_core::LineageError;

use crate::notes::list_notes;

/// Per-file coverage buckets. The distribution is what makes the section worth
/// printing: blended percentages hide that files cluster at 0% and 100%.
pub const COVERAGE_BUCKETS: [&str; 6] = ["0%", "1-25%", "25-50%", "50-75%", "75-99%", "100%"];

#[derive(Debug, Default, Clone)]
pub struct CoverageReport {
    pub commits_total: usize,
    pub commits_with_notes: usize,
    pub files_total: usize,
    pub files_with_any: usize,
    pub lines_total: u64,
    pub lines_covered: u64,
    /// Mean per-file coverage across covered files only — deliberately not the
    /// same statistic as `lines_covered / lines_total`, which a few large files
    /// would dominate.
    pub depth_within_covered: f64,
    pub histogram: [usize; COVERAGE_BUCKETS.len()],
}

/// Tracked paths at HEAD with their line counts, read from the committed tree
/// rather than the working directory so an uncommitted scratch file never
/// dilutes the denominator. Binary blobs are skipped: "lines" is meaningless
/// for them and line objects never point into one.
pub fn tracked_file_line_counts(repo: &Repository) -> Result<BTreeMap<String, u64>, LineageError> {
    let Ok(head) = repo.head() else {
        return Ok(BTreeMap::new());
    };
    let tree = head
        .peel_to_tree()
        .map_err(|e| LineageError::Other(e.to_string()))?;

    let mut counts = BTreeMap::new();
    let mut walk_error = None;
    tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
        if entry.kind() != Some(git2::ObjectType::Blob) {
            return git2::TreeWalkResult::Ok;
        }
        let Some(name) = entry.name() else {
            return git2::TreeWalkResult::Ok;
        };
        let path = format!("{dir}{name}");
        match repo.find_blob(entry.id()) {
            Ok(blob) => {
                if !blob.is_binary() {
                    counts.insert(path, count_lines(blob.content()));
                }
            }
            Err(e) => {
                walk_error = Some(LineageError::Other(e.to_string()));
                return git2::TreeWalkResult::Abort;
            }
        }
        git2::TreeWalkResult::Ok
    })
    .map_err(|e| LineageError::Other(e.to_string()))?;

    if let Some(e) = walk_error {
        return Err(e);
    }
    Ok(counts)
}

/// How many commits reachable from HEAD carry a lineage note. Capture health,
/// as the contrast the reach number is read against: high note coverage with
/// low file reach localizes the loss to materialization.
pub fn commit_note_coverage(repo: &Repository) -> Result<(usize, usize), LineageError> {
    let noted: std::collections::HashSet<String> = list_notes(repo)?
        .into_iter()
        .map(|note| note.commit_sha)
        .collect();

    let Ok(head) = repo.head() else {
        return Ok((0, 0));
    };
    let Ok(head_commit) = head.peel_to_commit() else {
        return Ok((0, 0));
    };
    let mut walk = repo
        .revwalk()
        .map_err(|e| LineageError::Other(e.to_string()))?;
    walk.push(head_commit.id())
        .map_err(|e| LineageError::Other(e.to_string()))?;

    let mut total = 0;
    let mut with_notes = 0;
    for oid in walk {
        let oid = oid.map_err(|e| LineageError::Other(e.to_string()))?;
        total += 1;
        if noted.contains(&oid.to_string()) {
            with_notes += 1;
        }
    }
    Ok((total, with_notes))
}

/// Combines tracked files with the line spans recorded against them. `spans`
/// comes from the search index (`coverage_spans`); files absent from it count
/// as zero-coverage rather than being skipped, since reach is the point.
pub fn summarize_coverage(
    tracked: &BTreeMap<String, u64>,
    spans: &BTreeMap<String, Vec<(u32, u32)>>,
    commits_total: usize,
    commits_with_notes: usize,
) -> CoverageReport {
    let mut report = CoverageReport {
        commits_total,
        commits_with_notes,
        ..Default::default()
    };
    let mut depth_sum = 0.0;

    for (path, &line_count) in tracked {
        if line_count == 0 {
            continue;
        }
        report.files_total += 1;
        report.lines_total += line_count;

        let covered = spans
            .get(path)
            .map_or(0, |file_spans| merged_line_count(file_spans, line_count));
        report.lines_covered += covered;

        let fraction = covered as f64 / line_count as f64;
        report.histogram[bucket_index(fraction)] += 1;
        if covered == 0 {
            continue;
        }
        report.files_with_any += 1;
        depth_sum += fraction;
    }

    if report.files_with_any > 0 {
        report.depth_within_covered = depth_sum / report.files_with_any as f64;
    }
    report
}

/// Union of the spans, in lines. A line object can outlive the file state it
/// was written against (the file shrank since), so spans are clamped to the
/// current line count first — without that, coverage exceeds 100%.
fn merged_line_count(spans: &[(u32, u32)], line_count: u64) -> u64 {
    let mut clamped: Vec<(u64, u64)> = spans
        .iter()
        .filter(|(start, _)| *start >= 1 && u64::from(*start) <= line_count)
        .map(|(start, end)| (u64::from(*start), u64::from(*end).min(line_count)))
        .collect();
    clamped.sort_unstable();

    let mut total = 0;
    let mut current: Option<(u64, u64)> = None;
    for (start, end) in clamped {
        let Some((cur_start, cur_end)) = current else {
            current = Some((start, end));
            continue;
        };
        // Adjacent spans (end + 1 == start) cover a contiguous region, so they
        // merge too rather than being counted as two.
        if start <= cur_end + 1 {
            current = Some((cur_start, cur_end.max(end)));
            continue;
        }
        total += cur_end - cur_start + 1;
        current = Some((start, end));
    }
    if let Some((cur_start, cur_end)) = current {
        total += cur_end - cur_start + 1;
    }
    total
}

/// Bucket boundaries follow the reference measurement: anything above zero but
/// below 1% still lands in `1-25%`, and only genuinely complete files reach
/// `100%`, so the bimodal split stays visible.
fn bucket_index(fraction: f64) -> usize {
    if fraction <= 0.0 {
        return 0;
    }
    if fraction >= 1.0 {
        return 5;
    }
    if fraction < 0.25 {
        return 1;
    }
    if fraction < 0.5 {
        return 2;
    }
    if fraction < 0.75 {
        return 3;
    }
    4
}

fn count_lines(content: &[u8]) -> u64 {
    content.iter().filter(|byte| **byte == b'\n').count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracked(entries: &[(&str, u64)]) -> BTreeMap<String, u64> {
        entries
            .iter()
            .map(|(path, count)| ((*path).to_string(), *count))
            .collect()
    }

    fn spans(entries: &[(&str, &[(u32, u32)])]) -> BTreeMap<String, Vec<(u32, u32)>> {
        entries
            .iter()
            .map(|(path, file_spans)| ((*path).to_string(), file_spans.to_vec()))
            .collect()
    }

    #[test]
    fn overlapping_and_adjacent_spans_are_counted_once() {
        assert_eq!(merged_line_count(&[(1, 10), (5, 20)], 100), 20);
        assert_eq!(merged_line_count(&[(1, 10), (11, 20)], 100), 20);
        assert_eq!(merged_line_count(&[(1, 10), (20, 25)], 100), 16);
        assert_eq!(merged_line_count(&[(7, 7)], 100), 1);
    }

    #[test]
    fn spans_outliving_a_shrunk_file_are_clamped() {
        // The file is now 5 lines; an object still claims 1-100.
        assert_eq!(merged_line_count(&[(1, 100)], 5), 5);
        // An object entirely past the new end contributes nothing.
        assert_eq!(merged_line_count(&[(50, 80)], 5), 0);
        // Mixed: only the surviving part counts.
        assert_eq!(merged_line_count(&[(1, 3), (50, 80)], 5), 3);
    }

    #[test]
    fn coverage_never_exceeds_the_file() {
        let report = summarize_coverage(
            &tracked(&[("a.rs", 5)]),
            &spans(&[("a.rs", &[(1, 100), (2, 400)])]),
            0,
            0,
        );
        assert_eq!(report.lines_total, 5);
        assert_eq!(report.lines_covered, 5);
        assert_eq!(report.histogram[5], 1);
    }

    #[test]
    fn reach_and_depth_are_separate_statistics() {
        // One fully covered small file, one barely covered large file: reach is
        // 2/2 but depth and line coverage differ sharply.
        let report = summarize_coverage(
            &tracked(&[("small.rs", 10), ("large.rs", 1000)]),
            &spans(&[("small.rs", &[(1, 10)]), ("large.rs", &[(1, 10)])]),
            0,
            0,
        );
        assert_eq!(report.files_with_any, 2);
        assert_eq!(report.lines_covered, 20);
        assert_eq!(report.lines_total, 1010);
        // Depth is the mean of 100% and 1%, not 20/1010.
        assert!((report.depth_within_covered - 0.505).abs() < 1e-9);
    }

    #[test]
    fn uncovered_files_land_in_the_zero_bucket_and_not_in_reach() {
        let report = summarize_coverage(
            &tracked(&[("a.rs", 10), ("b.rs", 10)]),
            &spans(&[("a.rs", &[(1, 10)])]),
            0,
            0,
        );
        assert_eq!(report.files_total, 2);
        assert_eq!(report.files_with_any, 1);
        assert_eq!(report.histogram[0], 1);
        assert_eq!(report.histogram[5], 1);
    }

    #[test]
    fn empty_files_are_excluded_from_every_denominator() {
        let report = summarize_coverage(&tracked(&[("empty.rs", 0)]), &spans(&[]), 0, 0);
        assert_eq!(report.files_total, 0);
        assert_eq!(report.lines_total, 0);
        assert_eq!(report.depth_within_covered, 0.0);
        assert_eq!(report.histogram, [0; 6]);
    }

    #[test]
    fn spans_for_untracked_paths_are_ignored() {
        let report = summarize_coverage(
            &tracked(&[("a.rs", 10)]),
            &spans(&[("a.rs", &[(1, 5)]), ("deleted.rs", &[(1, 999)])]),
            0,
            0,
        );
        assert_eq!(report.files_total, 1);
        assert_eq!(report.lines_covered, 5);
    }

    #[test]
    fn bucket_boundaries_split_partial_from_complete() {
        assert_eq!(bucket_index(0.0), 0);
        assert_eq!(bucket_index(0.001), 1);
        assert_eq!(bucket_index(0.25), 2);
        assert_eq!(bucket_index(0.5), 3);
        assert_eq!(bucket_index(0.75), 4);
        assert_eq!(bucket_index(0.99), 4);
        assert_eq!(bucket_index(1.0), 5);
    }

    #[test]
    fn line_counting_matches_newline_terminated_files() {
        assert_eq!(count_lines(b"a\nb\nc\n"), 3);
        assert_eq!(count_lines(b""), 0);
    }
}
