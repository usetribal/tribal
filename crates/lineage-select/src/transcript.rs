//! Folding a flat turn list into what a reader actually follows.
//!
//! Ported from the share page's `toTranscriptEntries`
//! (`apps/web/src/lib/transcript.ts`), whose rule was derived from measurements
//! over the real corpus. Keep the two in step: this is a deliberate duplication
//! of a non-obvious rule, recorded in the tech-debt register.
//!
//! The measurements that shaped it: 71% of turns carry no prose at all, because
//! a tool turn holds its output in a call result and an assistant turn that only
//! dispatched tools has an empty body. Rendered one row per turn that is a
//! column of blank badges. Assistant prose also comes in two kinds that read
//! nothing alike — narration threading one step to the next (median 99
//! characters) and a reply answering the person who asked (median 2,543) — so
//! the two cannot carry the same weight.

/// Who spoke. Mirrors the roles a conversation stores, kept local so this crate
/// needs no dependency on the conversation model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    User,
    Agent,
}

/// One turn, reduced to what the fold and the render need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptTurn {
    pub speaker: Speaker,
    pub content: String,
    /// Tool names this turn invoked, callers having already dropped results —
    /// a `tool_result` is the answer to a call, not a step of its own, and
    /// counting it would list the same action twice.
    pub tools: Vec<String>,
    /// Paths this turn wrote, excluding terminal commands.
    pub wrote: Vec<String>,
    /// Seconds since the session's first turn, when the turn is stamped.
    pub offset_seconds: Option<i64>,
}

impl TranscriptTurn {
    fn has_prose(&self) -> bool {
        !self.content.trim().is_empty()
    }

    fn has_activity(&self) -> bool {
        !self.tools.is_empty() || !self.wrote.is_empty()
    }

    /// A turn that did something but said nothing. These are the rows worth
    /// folding together: alone each is a bare badge, and consecutively they are
    /// the agent working rather than events a reader steps through.
    fn is_silent_activity(&self) -> bool {
        !self.has_prose() && self.has_activity()
    }
}

/// What the reader steps through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A turn with something to say — the spine a reader follows.
    Message {
        speaker: Speaker,
        content: String,
        /// The agent answering the person who asked, rather than narrating its
        /// way through a task. The answer the work was for.
        is_reply: bool,
        tools: Vec<String>,
    },
    /// A consecutive run of wordless tool work, kept whole.
    Activity { turns: Vec<TranscriptTurn> },
}

/// Split turns into messages and runs of activity.
///
/// A turn carrying prose is a message even when it also ran tools — the sentence
/// and the work it describes belong together. A turn with neither prose nor
/// activity has nothing to render and is dropped rather than shown as an empty
/// row.
pub fn fold(turns: &[TranscriptTurn]) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut run: Vec<TranscriptTurn> = Vec::new();

    for turn in turns {
        if turn.is_silent_activity() {
            run.push(turn.clone());
            continue;
        }
        // A turn with nothing at all does not break a run: the result of a tool
        // call arrives as its own turn, and treating that as a boundary would
        // split one stretch of work into an alternating column of one-step runs.
        if !turn.has_prose() && !turn.has_activity() {
            continue;
        }
        if !run.is_empty() {
            entries.push(Entry::Activity {
                turns: std::mem::take(&mut run),
            });
        }
        if turn.has_prose() || turn.has_activity() {
            entries.push(Entry::Message {
                speaker: turn.speaker,
                content: turn.content.clone(),
                is_reply: false,
                tools: turn.tools.clone(),
            });
        }
    }
    if !run.is_empty() {
        entries.push(Entry::Activity { turns: run });
    }
    mark_replies(&mut entries);
    entries
}

/// Mark each agent message that immediately precedes a user prompt, and the last
/// one in the session — a transcript that ends mid-answer still ends on an
/// answer.
///
/// Positional rather than a length threshold: a reply is the last thing the
/// agent says before the user speaks again, which holds for a one-line answer to
/// a one-line question that any size cutoff would misread.
fn mark_replies(entries: &mut [Entry]) {
    let mut since_user: Option<usize> = None;
    let mut to_mark = Vec::new();

    for (index, entry) in entries.iter().enumerate() {
        let Entry::Message { speaker, .. } = entry else {
            continue;
        };
        if *speaker == Speaker::User {
            to_mark.extend(since_user.take());
            continue;
        }
        since_user = Some(index);
    }
    to_mark.extend(since_user);

    for index in to_mark {
        if let Entry::Message { is_reply, .. } = &mut entries[index] {
            *is_reply = true;
        }
    }
}

/// One line describing a run, so a folded strip says what happened rather than
/// how many turns it hid.
///
/// Files written are named because that is the part a reader cares about; reads
/// are counted because which files were looked at rarely is.
pub fn activity_summary(turns: &[TranscriptTurn]) -> String {
    let steps: usize = turns.iter().map(|turn| turn.tools.len()).sum();
    let mut written: Vec<&str> = turns
        .iter()
        .flat_map(|turn| turn.wrote.iter().map(String::as_str))
        .collect();
    written.sort_unstable();
    written.dedup();

    let label = format!("{steps} {}", if steps == 1 { "step" } else { "steps" });
    match written.len() {
        0 => label,
        1 => format!("{label}, wrote {}", written[0]),
        many => format!("{label}, wrote {many} files"),
    }
}

/// Every tool named in a run, in order, with adjacent repeats collapsed.
pub fn activity_tools(turns: &[TranscriptTurn]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for turn in turns {
        for tool in &turn.tools {
            if names.last().map(String::as_str) != Some(tool.as_str()) {
                names.push(tool.clone());
            }
        }
    }
    names
}

/// How long a run took, when its turns are stamped.
///
/// Sub-second runs return nothing rather than "0s": the precision would be
/// noise, and an absent duration reads better than a meaningless one.
pub fn activity_duration(turns: &[TranscriptTurn]) -> Option<String> {
    let offsets: Vec<i64> = turns
        .iter()
        .filter_map(|turn| turn.offset_seconds)
        .collect();
    if offsets.len() < 2 {
        return None;
    }
    let span = offsets.iter().max()? - offsets.iter().min()?;
    if span < 1 {
        return None;
    }
    if span < 60 {
        return Some(format!("{span}s"));
    }
    if span < 3600 {
        return Some(format!("{}m", (span as f64 / 60.0).round() as i64));
    }
    Some(format!("{:.1}h", span as f64 / 3600.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(speaker: Speaker, content: &str) -> TranscriptTurn {
        TranscriptTurn {
            speaker,
            content: content.into(),
            tools: vec![],
            wrote: vec![],
            offset_seconds: None,
        }
    }

    fn worked(tools: &[&str], wrote: &[&str]) -> TranscriptTurn {
        TranscriptTurn {
            speaker: Speaker::Agent,
            content: String::new(),
            tools: tools.iter().map(|t| (*t).to_string()).collect(),
            wrote: wrote.iter().map(|w| (*w).to_string()).collect(),
            offset_seconds: None,
        }
    }

    #[test]
    fn wordless_tool_turns_fold_into_one_run() {
        let entries = fold(&[
            said(Speaker::User, "fix the guard"),
            worked(&["Read"], &[]),
            worked(&["Edit"], &["src/auth.rs"]),
            said(Speaker::Agent, "Done."),
        ]);
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[1], Entry::Activity { ref turns } if turns.len() == 2));
    }

    #[test]
    fn a_turn_with_prose_and_tools_stays_one_message() {
        let mut turn = said(Speaker::Agent, "Checking the other consumer");
        turn.tools = vec!["Read".into()];
        let entries = fold(&[turn]);
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0], Entry::Message { .. }));
    }

    #[test]
    fn a_turn_that_neither_speaks_nor_acts_is_dropped() {
        let entries = fold(&[said(Speaker::Agent, "   ")]);
        assert!(entries.is_empty());
    }

    #[test]
    fn an_empty_turn_between_two_runs_does_not_split_them() {
        // A tool result arrives as its own contentless turn; treating it as a
        // boundary turns one stretch of work into a column of one-step runs.
        let entries = fold(&[
            worked(&["Bash"], &[]),
            said(Speaker::Agent, ""),
            worked(&["Bash"], &[]),
        ]);
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0], Entry::Activity { ref turns } if turns.len() == 2));
    }

    #[test]
    fn the_last_agent_message_before_the_user_speaks_is_the_reply() {
        let entries = fold(&[
            said(Speaker::User, "why?"),
            said(Speaker::Agent, "Let me look"),
            said(Speaker::Agent, "Because the salt is null."),
            said(Speaker::User, "thanks"),
        ]);
        let replies: Vec<bool> = entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Message {
                    speaker: Speaker::Agent,
                    is_reply,
                    ..
                } => Some(*is_reply),
                _ => None,
            })
            .collect();
        assert_eq!(replies, vec![false, true], "narration is not a reply");
    }

    #[test]
    fn a_transcript_ending_mid_answer_still_ends_on_a_reply() {
        let entries = fold(&[
            said(Speaker::User, "why?"),
            said(Speaker::Agent, "Because the salt is null."),
        ]);
        assert!(matches!(
            entries.last(),
            Some(Entry::Message { is_reply: true, .. })
        ));
    }

    #[test]
    fn a_one_line_answer_to_a_one_line_question_is_still_a_reply() {
        let entries = fold(&[said(Speaker::User, "ok?"), said(Speaker::Agent, "yes")]);
        assert!(matches!(entries[1], Entry::Message { is_reply: true, .. }));
    }

    #[test]
    fn a_run_names_one_written_file_and_counts_many() {
        assert_eq!(
            activity_summary(&[worked(&["Edit"], &["src/auth.rs"])]),
            "1 step, wrote src/auth.rs"
        );
        assert_eq!(
            activity_summary(&[worked(&["Edit", "Edit"], &["a.rs", "b.rs"])]),
            "2 steps, wrote 2 files"
        );
        assert_eq!(activity_summary(&[worked(&["Read"], &[])]), "1 step");
    }

    #[test]
    fn adjacent_repeats_of_a_tool_collapse() {
        let tools = activity_tools(&[
            worked(&["Read", "Read", "Edit"], &[]),
            worked(&["Edit"], &[]),
        ]);
        assert_eq!(tools, vec!["Read", "Edit"]);
    }

    #[test]
    fn a_run_shorter_than_a_second_reports_no_duration() {
        let mut first = worked(&["Read"], &[]);
        first.offset_seconds = Some(10);
        let mut second = worked(&["Edit"], &[]);
        second.offset_seconds = Some(10);
        assert_eq!(activity_duration(&[first, second]), None);
    }

    #[test]
    fn run_durations_read_in_the_largest_useful_unit() {
        let at = |seconds: i64| {
            let mut turn = worked(&["Read"], &[]);
            turn.offset_seconds = Some(seconds);
            turn
        };
        assert_eq!(activity_duration(&[at(0), at(45)]).as_deref(), Some("45s"));
        assert_eq!(activity_duration(&[at(0), at(600)]).as_deref(), Some("10m"));
        assert_eq!(
            activity_duration(&[at(0), at(9000)]).as_deref(),
            Some("2.5h")
        );
    }
}
