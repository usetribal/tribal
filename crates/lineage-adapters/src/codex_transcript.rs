//! `Conversation` -> Codex CLI rollout JSONL.
//!
//! Codex resolves `codex resume <id>` by scanning `~/.codex/sessions/` for a
//! rollout whose `session_meta` names the id — no registry and no server
//! handshake — so a rollout written there is resumable on an installation that has
//! never seen the session. Verified against codex-cli 0.153.4.
//!
//! Tool activity is narrated as prose rather than replayed as `function_call`
//! records; `transcript_writing` holds that rule and the reasons for it.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, SecondsFormat, Utc};
use lineage_agent::RenderedTranscript;
use lineage_core::Conversation;
use serde_json::{json, Value};

use crate::transcript_writing::{mint_uuid, narrate, Speaker};

/// Codex's own session ids are v7: time-ordered, which is also how it sorts them.
const SESSION_ID_VERSION: u8 = 7;

/// Names the writer honestly, as `cli_version` below does. Codex refuses a
/// `session_meta` without a `cli_version` ("does not start with session
/// metadata"), and tribal's reader accepts any rollout that states an originator.
const ORIGINATOR: &str = "tribal";

/// Every record carries its position. Codex refuses to resume a rollout whose last
/// record lacks one ("final paginated rollout record … is missing an ordinal"), and
/// every record it writes itself has one.
struct Rollout {
    lines: Vec<String>,
}

impl Rollout {
    fn push(&mut self, kind: &str, timestamp: DateTime<Utc>, payload: Value) {
        let record = json!({
            "timestamp": timestamp.to_rfc3339_opts(SecondsFormat::Millis, true),
            "ordinal": self.lines.len(),
            "type": kind,
            "payload": payload,
        });
        self.lines.push(record.to_string());
    }

    fn into_jsonl(self) -> String {
        let mut out = self.lines.join("\n");
        out.push('\n');
        out
    }
}

/// Renders `conversation` as a resumable Codex rollout under `home`.
///
/// `workspace_root` becomes the session's `cwd`: tribal's import keeps only the
/// rollouts whose `cwd` is the workspace, so a rollout written for one workspace
/// is invisible from another. `now` dates the new session and names its file.
pub fn render_codex_transcript(
    conversation: &Conversation,
    home: &Path,
    workspace_root: &Path,
    now: DateTime<Utc>,
) -> RenderedTranscript {
    let session_id = mint_uuid(SESSION_ID_VERSION);
    // Collecting the components drops the trailing separator a git workdir path
    // carries, so `cwd` reads as the directory itself rather than a variant of it.
    let cwd: PathBuf = workspace_root.components().collect();
    let mut rollout = Rollout { lines: Vec::new() };
    rollout.push(
        "session_meta",
        now,
        json!({
            "id": session_id,
            "timestamp": now.to_rfc3339_opts(SecondsFormat::Millis, true),
            "cwd": cwd.display().to_string(),
            "originator": ORIGINATOR,
            "cli_version": env!("CARGO_PKG_VERSION"),
        }),
    );

    for turn in &conversation.turns {
        let timestamp = turn.timestamp.unwrap_or(conversation.started_at);
        for (speaker, text) in narrate(turn) {
            rollout.push("response_item", timestamp, message(speaker, &text));
        }
    }

    RenderedTranscript {
        path: transcript_path(home, &session_id, now),
        resume_command: format!("codex resume {session_id}"),
        // Codex finds the rollout by id from anywhere, but the resumed agent works
        // in whatever directory it is launched from.
        resume_cwd: workspace_root.to_path_buf(),
        contents: rollout.into_jsonl(),
        session_handle: session_id,
    }
}

/// Codex files a rollout by the local date and time it started, in the same
/// layout it uses for its own sessions, so a written one sorts among them.
pub fn transcript_path(home: &Path, session_id: &str, now: DateTime<Utc>) -> PathBuf {
    let local = now.with_timezone(&Local);
    home.join(".codex")
        .join("sessions")
        .join(local.format("%Y").to_string())
        .join(local.format("%m").to_string())
        .join(local.format("%d").to_string())
        .join(format!(
            "rollout-{}-{session_id}.jsonl",
            local.format("%Y-%m-%dT%H-%M-%S")
        ))
}

/// The Responses API types text by who wrote it: input from the user, output from
/// the model.
fn message(speaker: Speaker, text: &str) -> Value {
    let content_type = match speaker {
        Speaker::User => "input_text",
        Speaker::Assistant => "output_text",
    };
    json!({
        "type": "message",
        "role": speaker.as_str(),
        "content": [{ "type": content_type, "text": text }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use lineage_core::{AgentKind, LineageId, Role, ToolCall, ToolTarget, ToolTargetKind, Turn};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 27, 21, 5, 9).unwrap()
    }

    fn turn(role: Role, content: &str) -> Turn {
        Turn {
            id: LineageId::new(),
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            model: None,
            timestamp: Some(Utc.with_ymd_and_hms(2026, 9, 7, 8, 38, 20).unwrap()),
            artifacts: Vec::new(),
        }
    }

    fn render(turns: Vec<Turn>) -> RenderedTranscript {
        let mut conv = Conversation::new(AgentKind::Codex, "/elsewhere/alice/repo");
        conv.turns = turns;
        render_codex_transcript(
            &conv,
            Path::new("/home/bob"),
            Path::new("/work/repo"),
            now(),
        )
    }

    fn records(rendered: &RenderedTranscript) -> Vec<Value> {
        rendered
            .contents
            .lines()
            .map(|l| serde_json::from_str(l).expect("record is valid json"))
            .collect()
    }

    #[test]
    fn the_first_record_names_the_session_and_the_workspace_it_resumes_in() {
        let rendered = render(vec![turn(Role::User, "hello")]);
        let meta = &records(&rendered)[0];

        assert_eq!(meta["type"], "session_meta");
        assert_eq!(meta["payload"]["id"], rendered.session_handle.as_str());
        // The forker's workspace, not the source session's: the source path is
        // somebody else's machine.
        assert_eq!(meta["payload"]["cwd"], "/work/repo");
        assert_eq!(meta["payload"]["originator"], ORIGINATOR);
        assert!(meta["payload"]["cli_version"].is_string());
    }

    #[test]
    fn a_trailing_separator_on_the_workspace_is_not_carried_into_cwd() {
        let conv = Conversation::new(AgentKind::Codex, "/work/repo");
        let rendered = render_codex_transcript(
            &conv,
            Path::new("/home/bob"),
            Path::new("/work/repo/"),
            now(),
        );

        assert_eq!(records(&rendered)[0]["payload"]["cwd"], "/work/repo");
    }

    #[test]
    fn every_record_carries_a_timestamp_and_its_position() {
        let rendered = render(vec![turn(Role::User, "hello"), turn(Role::Assistant, "hi")]);

        for (position, record) in records(&rendered).iter().enumerate() {
            assert_eq!(record["ordinal"], position);
            assert!(record["timestamp"].as_str().unwrap().ends_with('Z'));
            assert!(record["payload"].is_object());
        }
    }

    #[test]
    fn messages_are_typed_by_who_wrote_them() {
        let rendered = render(vec![
            turn(Role::User, "fix it"),
            turn(Role::Assistant, "fixed"),
        ]);
        let records = records(&rendered);

        assert_eq!(records[1]["type"], "response_item");
        assert_eq!(records[1]["payload"]["role"], "user");
        assert_eq!(records[1]["payload"]["content"][0]["type"], "input_text");
        assert_eq!(records[1]["payload"]["content"][0]["text"], "fix it");
        assert_eq!(records[2]["payload"]["role"], "assistant");
        assert_eq!(records[2]["payload"]["content"][0]["type"], "output_text");
        assert_eq!(records[1]["timestamp"], "2026-09-07T08:38:20.000Z");
    }

    #[test]
    fn tool_activity_is_narrated_rather_than_replayed() {
        let mut tool = turn(Role::Tool, r#"exec_command("{\"cmd\":\"pwd\"}")"#);
        tool.tool_calls.push(ToolCall {
            id: "call-1".into(),
            name: "exec_command".into(),
            arguments: r#""{\"cmd\":\"pwd\"}""#.into(),
            result: Some("/work/repo".into()),
            target: Some(ToolTarget {
                kind: ToolTargetKind::Command,
                value: "pwd".into(),
            }),
        });
        let rendered = render(vec![turn(Role::User, "where am I"), tool]);

        assert!(!rendered.contents.contains("function_call"));
        let texts: Vec<String> = records(&rendered)
            .iter()
            .skip(1)
            .map(|r| {
                r["payload"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(texts.len(), 3);
        assert!(texts[1].contains("- exec_command pwd"), "{}", texts[1]);
        assert!(texts[2].ends_with("/work/repo"), "{}", texts[2]);
    }

    #[test]
    fn the_file_is_filed_by_local_date_and_named_for_the_session() {
        let rendered = render(vec![turn(Role::User, "hello")]);
        let local = now().with_timezone(&Local);

        let expected_dir =
            Path::new("/home/bob/.codex/sessions").join(local.format("%Y/%m/%d").to_string());
        assert_eq!(rendered.path.parent().unwrap(), expected_dir);
        assert_eq!(
            rendered.path.file_name().unwrap().to_str().unwrap(),
            format!(
                "rollout-{}-{}.jsonl",
                local.format("%Y-%m-%dT%H-%M-%S"),
                rendered.session_handle
            )
        );
    }

    #[test]
    fn a_fresh_v7_session_id_is_minted_every_render() {
        let first = render(vec![turn(Role::User, "hello")]);
        let second = render(vec![turn(Role::User, "hello")]);

        assert_ne!(first.session_handle, second.session_handle);
        assert_eq!(first.session_handle.chars().nth(14), Some('7'));
    }

    /// The caller prints this verbatim, so the id in it has to be the id the file
    /// was written under.
    #[test]
    fn the_resume_command_names_the_minted_handle() {
        let rendered = render(vec![turn(Role::User, "hello")]);

        assert_eq!(
            rendered.resume_command,
            format!("codex resume {}", rendered.session_handle)
        );
        assert_eq!(rendered.resume_cwd, Path::new("/work/repo"));
    }

    #[test]
    fn system_and_empty_turns_are_dropped() {
        let rendered = render(vec![
            turn(Role::User, "hello"),
            turn(Role::System, "tribal internal note"),
            turn(Role::Assistant, "   "),
        ]);

        assert_eq!(records(&rendered).len(), 2);
    }
}
