//! `git lineage pull` — bring teammates' sessions down from a Lineage server
//! into this repository's lineage refs.
//!
//! Pull is not sync with the arrows reversed. Push merges into an authority;
//! pull merges into a cache. Three rules follow from that, and they mirror the
//! four write properties in `docs/sync-semantics.md` so push and pull compose:
//!
//! **Pull never deletes.** A conversation the server does not mention is left
//! exactly as it is. This machine may hold sessions it has never pushed, and
//! reconciling by deletion would destroy unsynced local work.
//!
//! **Container fields merge monotonically.** `commit_shas` is a set union,
//! `ended_at` is a max, the turn set is grow-only, and `metadata` is
//! first-write-wins per key — the same functions in both directions, which is
//! what makes Alice-push-then-Bob-pull converge to the same state as the
//! reverse. Turns are content-addressed and immutable, so re-pulling one is a
//! no-op by construction and a second identical pull writes nothing.
//!
//! **`pull_origin` is first-write-wins.** A session already here keeps the
//! marker it already had; re-pulling does not restamp it. The marker records
//! where a session came from, and that fact does not change because the same
//! server served it again.
//!
//! The cursor is a content digest, not a sequence number or a timestamp
//! watermark: the client says what it holds (`{id, turn count, ended_at}`) and
//! the server answers with what differs. No gapless counter to get wrong under
//! concurrency, no clock to trust.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use lineage_core::{
    merge_commit_shas, merge_ended_at, AgentKind, Conversation, LineageId, PullOrigin, Role, Turn,
    CONVERSATION_SCHEMA,
};
use lineage_git::{
    commit_time, list_session_ids, open_repo, persist_conversation, read_conversation_stored,
    resolve_repo_binding, LineageRepo,
};
use serde::{Deserialize, Serialize};

use crate::auth;
use crate::commands::index_persisted_sessions_best_effort;
use crate::events::{EventLog, Outcome};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const PULL_TIMEOUT: Duration = Duration::from_secs(120);

/// Conversations listed by name before the tail is summarised as a count. Long
/// enough to see what arrived, short enough not to bury the summary line.
const CONVERSATIONS_SHOWN: usize = 10;

// --- Wire types (`packages/contracts/src/pull.ts`) ------------------------------

/// One locally-held conversation as the client describes it during negotiation.
///
/// Turn count and `ended_at` are the whole digest: turns are immutable and
/// content-hashed, so a count detects the grow-only turn set changing, and
/// `ended_at` covers the container fields that merge monotonically.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HaveEntry {
    pub conversation_id: String,
    pub turn_count: usize,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NegotiateRequest {
    pub repo: String,
    pub have: Vec<HaveEntry>,
}

/// Why the server wants to send a conversation. Diagnostic rather than
/// load-bearing — the client fetches every entry regardless — but it is what
/// lets a pull explain itself as "3 new, 1 grown" instead of a bare count.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WantEntry {
    pub conversation_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NegotiateResponse {
    #[serde(default)]
    pub want: Vec<WantEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchRequest {
    pub repo: String,
    pub conversation_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PulledTurn {
    pub id: String,
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub content_truncated: bool,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PulledConversation {
    pub id: String,
    pub agent: String,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub parent_session_id: Option<String>,
    #[serde(default)]
    pub prompted_by_name: Option<String>,
    #[serde(default)]
    pub commit_shas: Vec<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub turns: Vec<PulledTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchResponse {
    #[serde(default)]
    pub conversations: Vec<PulledConversation>,
}

/// The two server calls, behind a trait so the merge and ref-writing halves can
/// be tested without a server. Everything above this line is pure.
pub trait PullTransport {
    fn negotiate(&self, request: &NegotiateRequest) -> Result<NegotiateResponse>;
    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse>;
}

/// `ureq` over bearer auth, matching the push transport in `lineage-git::sync`.
pub struct HttpTransport {
    base: String,
    token: String,
}

impl HttpTransport {
    pub fn new(server: &str, token: &str) -> Self {
        Self {
            base: server.trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }

    fn post<Req: Serialize, Res: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        request: &Req,
    ) -> Result<Res> {
        let url = format!("{}{path}", self.base);
        let response = ureq::post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .timeout(PULL_TIMEOUT)
            .send_json(serde_json::to_value(request)?)
            .map_err(|e| format!("pull request to {path} failed: {e}"))?;
        if !(200..300).contains(&response.status()) {
            let status = response.status();
            let text = response.into_string().unwrap_or_default();
            return Err(format!("pull {path} HTTP {status}: {text}").into());
        }
        Ok(response.into_json()?)
    }
}

impl PullTransport for HttpTransport {
    fn negotiate(&self, request: &NegotiateRequest) -> Result<NegotiateResponse> {
        self.post("/v0/pull/negotiate", request)
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse> {
        self.post("/v0/pull/fetch", request)
    }
}

// --- Pure digest and merge -----------------------------------------------------

/// What this machine holds, one entry per stored conversation. Sorted by id so
/// two clients holding the same sessions send byte-identical digests, which
/// makes a request diffable when a negotiation misbehaves.
pub fn local_digest(conversations: &[Conversation]) -> Vec<HaveEntry> {
    let mut entries: Vec<HaveEntry> = conversations
        .iter()
        .map(|conv| HaveEntry {
            conversation_id: conv.id.to_string(),
            turn_count: conv.turns.len(),
            ended_at: conv.ended_at,
        })
        .collect();
    entries.sort_by(|a, b| a.conversation_id.cmp(&b.conversation_id));
    entries
}

/// The monotonic container merge, in one place because a second divergent copy
/// is what would silently break order-independence across push and pull.
///
/// `local` is `None` for a session this machine has never seen; the incoming
/// copy then becomes the whole session. Otherwise every field grows or holds:
/// nothing the local copy has can be lost by pulling.
pub fn merge_pulled(
    local: Option<Conversation>,
    incoming: &PulledConversation,
    origin: &PullOrigin,
) -> Conversation {
    let mut merged = match local {
        Some(existing) => existing,
        None => new_from_pulled(incoming),
    };

    merged.ended_at = merge_ended_at(merged.ended_at, incoming.ended_at);
    merge_commit_shas(&mut merged.commit_shas, &incoming.commit_shas);
    merge_metadata(&mut merged.metadata, incoming);
    merge_turns(&mut merged.turns, &incoming.turns);

    // First-write-wins: a session already marked keeps its marker. Restamping
    // would rewrite the record of where it first came from on every pull.
    if merged.pull_origin.is_none() {
        merged.pull_origin = Some(origin.clone());
    }
    merged
}

/// The skeleton of a conversation this machine has never held. Fields the wire
/// does not carry (turns, commit shas, metadata) are filled by the merge.
fn new_from_pulled(incoming: &PulledConversation) -> Conversation {
    Conversation {
        schema_version: CONVERSATION_SCHEMA.into(),
        id: LineageId::from(incoming.id.clone()),
        agent: agent_kind(&incoming.agent),
        started_at: incoming.started_at,
        ended_at: incoming.ended_at,
        // The server does not know this machine's checkout, and a pulled
        // session was never run here. Naming a local path would be a claim
        // about where the work happened that nothing here can support.
        workspace_root: String::new(),
        parent_session_id: incoming
            .parent_session_id
            .as_ref()
            .map(|id| LineageId::from(id.clone())),
        fork_origin: None,
        pull_origin: None,
        // A private session is never emitted by the server, so anything that
        // arrives here is shareable by construction.
        private: false,
        turns: Vec::new(),
        commit_shas: Vec::new(),
        metadata: Default::default(),
    }
}

/// Max, treating absence as "no end recorded yet" so a local copy that knows an
/// end time is never regressed to unknown by a server copy that does not.
/// First-write-wins per key. The local copy is the first writer for anything it
/// already holds, so a pull adds keys and never overwrites them.
fn merge_metadata(
    local: &mut std::collections::HashMap<String, serde_json::Value>,
    incoming: &PulledConversation,
) {
    for (key, value) in &incoming.metadata {
        local.entry(key.clone()).or_insert_with(|| value.clone());
    }

    // `prompted_by_name` and `model` are promoted to their own wire fields for
    // display, but locally they live in metadata like every other adapter
    // extra — so fold them back rather than inventing a second place to look.
    if let Some(name) = &incoming.prompted_by_name {
        local
            .entry(lineage_git::PROMPTED_BY_NAME.to_string())
            .or_insert_with(|| serde_json::Value::String(name.clone()));
    }
    if let Some(model) = &incoming.model {
        local
            .entry("model".to_string())
            .or_insert_with(|| serde_json::Value::String(model.clone()));
    }
}

/// Grow-only union keyed by turn id. A turn that happened never changes, so a
/// turn already held is left untouched — which is what makes a re-pull a no-op
/// even though the server resends the whole conversation.
fn merge_turns(local: &mut Vec<Turn>, incoming: &[PulledTurn]) {
    let held: BTreeSet<String> = local.iter().map(|turn| turn.id.to_string()).collect();
    for turn in incoming {
        if held.contains(&turn.id) {
            continue;
        }
        local.push(to_turn(turn));
    }
}

fn to_turn(pulled: &PulledTurn) -> Turn {
    Turn {
        id: LineageId::from(pulled.id.clone()),
        role: role_of(&pulled.role),
        content: pulled.content.clone(),
        // Tool calls and artifacts are not on the pull wire yet. Leaving them
        // empty is honest; a fork renders tool activity as prose regardless,
        // so a pulled session still reads as history.
        tool_calls: Vec::new(),
        model: pulled.model.clone(),
        timestamp: pulled.timestamp,
        artifacts: Vec::new(),
    }
}

/// The wire carries role and agent as free strings. An unrecognised value maps
/// to the closest local kind rather than failing the pull: refusing a whole
/// conversation because one turn had a role this build has not learned yet
/// would lose more than it protects.
fn role_of(role: &str) -> Role {
    match role.to_ascii_lowercase().as_str() {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        _ => Role::System,
    }
}

fn agent_kind(agent: &str) -> AgentKind {
    match agent.to_ascii_lowercase().as_str() {
        "cursor" => AgentKind::Cursor,
        "codex" => AgentKind::Codex,
        _ => AgentKind::Claude,
    }
}

/// True when merging changed nothing this machine will store — the second of
/// two identical pulls. Compared on the fields the merge can move, so an
/// unrelated local edit does not read as a pull effect.
pub fn is_noop(before: Option<&Conversation>, after: &Conversation) -> bool {
    let Some(before) = before else {
        return false;
    };
    before.turns.len() == after.turns.len()
        && before.ended_at == after.ended_at
        && before.commit_shas == after.commit_shas
        && before.pull_origin == after.pull_origin
        && before.metadata.len() == after.metadata.len()
}

// --- Command -------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct PullReport {
    pub wanted: usize,
    pub written: Vec<String>,
    pub unchanged: usize,
    pub reasons: BTreeMap<String, usize>,
}

pub fn pull(
    repo_path: &Path,
    server: Option<&str>,
    token: Option<&str>,
    remote: &str,
    dry_run: bool,
) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let server = auth::resolve_server(server)?;
    let token = resolve_pull_token(&server, token)?;

    let binding = resolve_repo_binding(repo.inner(), remote)?;
    let transport = HttpTransport::new(&server, &token);
    let report = run_pull(
        &repo,
        &transport,
        &binding.normalized_remote_url,
        &pull_origin_for(&server),
        dry_run,
    )?;

    print_report(&report, &server, dry_run);
    EventLog::for_git_dir(&repo.git_dir()).append(
        Utc::now(),
        "pull",
        Outcome::Ok,
        serde_json::json!({
            "server": server,
            "remote": remote,
            "repo": binding.normalized_remote_url,
            "dry_run": dry_run,
            "wanted": report.wanted,
            "written": report.written.len(),
            "unchanged": report.unchanged,
            "reasons": report.reasons,
        }),
    );
    Ok(())
}

/// The whole flow with the transport injected: digest, negotiate, fetch, merge,
/// write. Separate from [`pull`] so tests drive it with a stub server against a
/// real tempfile repo — the merge and the ref writing are the parts worth
/// testing, and neither needs a network.
pub fn run_pull(
    repo: &LineageRepo,
    transport: &dyn PullTransport,
    repo_url: &str,
    origin: &PullOrigin,
    dry_run: bool,
) -> Result<PullReport> {
    let local = load_local_conversations(repo)?;
    let negotiated = transport.negotiate(&NegotiateRequest {
        repo: repo_url.to_string(),
        have: local_digest(&local),
    })?;

    let mut report = PullReport {
        wanted: negotiated.want.len(),
        ..PullReport::default()
    };
    for want in &negotiated.want {
        *report.reasons.entry(want.reason.clone()).or_insert(0) += 1;
    }
    if negotiated.want.is_empty() {
        return Ok(report);
    }

    let fetched = transport.fetch(&FetchRequest {
        repo: repo_url.to_string(),
        conversation_ids: negotiated
            .want
            .iter()
            .map(|want| want.conversation_id.clone())
            .collect(),
    })?;

    let mut persisted: Vec<LineageId> = Vec::new();
    for incoming in &fetched.conversations {
        let incoming = &drop_absent_commits(repo, incoming)?;
        let existing =
            read_conversation_stored(repo.inner(), &LineageId::from(incoming.id.clone()))?;
        let merged = merge_pulled(existing.clone(), incoming, origin);
        if is_noop(existing.as_ref(), &merged) {
            report.unchanged += 1;
            continue;
        }
        if !dry_run {
            persist_conversation(repo.inner(), &merged)?;
            persisted.push(merged.id.clone());
        }
        report.written.push(incoming.id.clone());
    }

    // Indexed once for the batch rather than per session: opening the index is
    // the fixed cost, and a pull brings down many sessions at a time. Best
    // effort — the sessions are already on disk, and a pull that reported
    // failure after writing them would be lying about what landed.
    index_persisted_sessions_best_effort(repo, &persisted);
    Ok(report)
}

/// Drops commit shas this repository does not have yet.
///
/// The server's copy names every commit the session reached, including ones on
/// branches this checkout has never fetched. Storing them would break the write
/// path — persisting a session materializes line objects per commit, and that
/// walk fails on a commit absent from the object database. Dropping is safe
/// under the monotonic rule: the sha is not lost, it simply arrives on the next
/// pull after `git fetch`, and the union merge adds it then.
fn drop_absent_commits(
    repo: &LineageRepo,
    incoming: &PulledConversation,
) -> Result<PulledConversation> {
    let mut present = Vec::new();
    for sha in &incoming.commit_shas {
        if commit_time(repo.inner(), sha)?.is_some() {
            present.push(sha.clone());
        }
    }
    if present.len() == incoming.commit_shas.len() {
        return Ok(incoming.clone());
    }
    Ok(PulledConversation {
        commit_shas: present,
        ..incoming.clone()
    })
}

fn load_local_conversations(repo: &LineageRepo) -> Result<Vec<Conversation>> {
    let mut conversations = Vec::new();
    for id in list_session_ids(repo.inner())? {
        if let Some(conv) = read_conversation_stored(repo.inner(), &id)? {
            conversations.push(conv);
        }
    }
    Ok(conversations)
}

fn pull_origin_for(server: &str) -> PullOrigin {
    PullOrigin {
        server: server.to_string(),
        // The server names the tenant it served from; the client never picks
        // one, and the wire does not carry it back, so it stays unset here.
        tenant: None,
        pulled_at: Utc::now(),
        lineage_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Same resolution order as `sync`: explicit flag, then `LINEAGE_TOKEN`, then
/// the stored login.
fn resolve_pull_token(server: &str, token: Option<&str>) -> Result<String> {
    let explicit = token
        .map(str::to_string)
        .filter(|t| !t.is_empty())
        .or_else(|| {
            std::env::var("LINEAGE_TOKEN")
                .ok()
                .filter(|t| !t.is_empty())
        });
    match explicit {
        Some(token) => Ok(token),
        None => auth::access_token_for(server),
    }
}

/// Honest about what happened, in `fork`'s register: say what the server had,
/// what changed here, and what is still true of the local copy.
fn print_report(report: &PullReport, server: &str, dry_run: bool) {
    if report.wanted == 0 {
        println!("Already up to date with {server}.");
        return;
    }

    println!(
        "{server} has {} conversation(s) this repository is missing or behind on{}.",
        report.wanted,
        describe_reasons(&report.reasons)
    );

    if report.written.is_empty() {
        println!("Nothing changed locally — every one of them was already here in full.");
        return;
    }

    let verb = if dry_run { "Would write" } else { "Wrote" };
    println!("{verb} {} session(s):", report.written.len());
    for id in report.written.iter().take(CONVERSATIONS_SHOWN) {
        println!("    {id}");
    }
    if report.written.len() > CONVERSATIONS_SHOWN {
        println!("    +{} more", report.written.len() - CONVERSATIONS_SHOWN);
    }
    if report.unchanged > 0 {
        println!(
            "{} were already here in full and were left alone.",
            report.unchanged
        );
    }
    println!();

    if dry_run {
        println!("Nothing was written (--dry-run).");
        return;
    }
    println!("Pull never deletes: sessions the server did not mention are untouched,");
    println!("and turns you already had were kept as they were.");
    println!();
    println!("`git lineage list` shows them; `git lineage fork <id>` continues one.");
}

fn describe_reasons(reasons: &BTreeMap<String, usize>) -> String {
    if reasons.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = reasons
        .iter()
        .map(|(reason, count)| format!("{count} {reason}"))
        .collect();
    format!(" ({})", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pulled_turn(id: &str, content: &str) -> PulledTurn {
        PulledTurn {
            id: id.into(),
            role: "user".into(),
            content: content.into(),
            content_truncated: false,
            model: None,
            timestamp: None,
        }
    }

    fn incoming(id: &str, turns: Vec<PulledTurn>) -> PulledConversation {
        PulledConversation {
            id: id.into(),
            agent: "claude".into(),
            started_at: "2026-07-01T00:00:00Z".parse().unwrap(),
            ended_at: None,
            model: None,
            parent_session_id: None,
            prompted_by_name: None,
            commit_shas: vec![],
            metadata: BTreeMap::new(),
            turns,
        }
    }

    fn origin() -> PullOrigin {
        PullOrigin {
            server: "https://api.example.dev".into(),
            tenant: None,
            pulled_at: "2026-07-26T00:00:00Z".parse().unwrap(),
            lineage_version: "0.0.0".into(),
        }
    }

    #[test]
    fn digest_is_id_turn_count_and_end_time_sorted_by_id() {
        let mut later = Conversation::new(AgentKind::Claude, "/tmp/a");
        later.id = LineageId::from("zzz");
        later.turns.push(to_turn(&pulled_turn("zzz-0", "hi")));
        let mut earlier = Conversation::new(AgentKind::Claude, "/tmp/b");
        earlier.id = LineageId::from("aaa");
        earlier.ended_at = Some("2026-07-02T00:00:00Z".parse().unwrap());

        let digest = local_digest(&[later, earlier]);
        assert_eq!(digest[0].conversation_id, "aaa");
        assert_eq!(digest[0].turn_count, 0);
        assert!(digest[0].ended_at.is_some());
        assert_eq!(digest[1].conversation_id, "zzz");
        assert_eq!(digest[1].turn_count, 1);
    }

    #[test]
    fn a_session_this_machine_never_had_arrives_whole() {
        let merged = merge_pulled(
            None,
            &incoming("s1", vec![pulled_turn("s1-0", "hi")]),
            &origin(),
        );
        assert_eq!(merged.id.as_str(), "s1");
        assert_eq!(merged.turns.len(), 1);
        assert_eq!(merged.pull_origin.as_ref().unwrap().server, origin().server);
    }

    #[test]
    fn turns_grow_and_never_shrink() {
        let mut local = Conversation::new(AgentKind::Claude, "/tmp/a");
        local.id = LineageId::from("s1");
        local.turns.push(to_turn(&pulled_turn("s1-0", "hi")));
        local.turns.push(to_turn(&pulled_turn("s1-1", "and more")));

        // The server's copy is stale — it has only the first turn.
        let merged = merge_pulled(
            Some(local),
            &incoming("s1", vec![pulled_turn("s1-0", "hi")]),
            &origin(),
        );
        assert_eq!(merged.turns.len(), 2);
    }

    #[test]
    fn commit_shas_union_and_ended_at_takes_the_later_time() {
        let mut local = Conversation::new(AgentKind::Claude, "/tmp/a");
        local.id = LineageId::from("s1");
        local.commit_shas.push("aaa".into());
        local.ended_at = Some("2026-07-05T00:00:00Z".parse().unwrap());

        let mut server_copy = incoming("s1", vec![]);
        server_copy.commit_shas = vec!["aaa".into(), "bbb".into()];
        server_copy.ended_at = Some("2026-07-03T00:00:00Z".parse().unwrap());

        let merged = merge_pulled(Some(local), &server_copy, &origin());
        assert_eq!(merged.commit_shas, vec!["aaa", "bbb"]);
        assert_eq!(
            merged.ended_at,
            Some("2026-07-05T00:00:00Z".parse().unwrap())
        );
    }

    #[test]
    fn merging_the_same_response_twice_changes_nothing_the_second_time() {
        let server_copy = incoming("s1", vec![pulled_turn("s1-0", "hi")]);
        let first = merge_pulled(None, &server_copy, &origin());
        let second = merge_pulled(Some(first.clone()), &server_copy, &origin());
        assert!(is_noop(Some(&first), &second));
    }

    #[test]
    fn an_existing_pull_marker_is_not_restamped() {
        let first = merge_pulled(None, &incoming("s1", vec![]), &origin());

        let mut later = origin();
        later.server = "https://other.example.dev".into();
        let second = merge_pulled(Some(first), &incoming("s1", vec![]), &later);

        assert_eq!(
            second.pull_origin.as_ref().unwrap().server,
            "https://api.example.dev"
        );
    }

    #[test]
    fn metadata_already_here_wins_over_the_server_copy() {
        let mut local = Conversation::new(AgentKind::Claude, "/tmp/a");
        local.id = LineageId::from("s1");
        local.metadata.insert(
            lineage_git::PROMPTED_BY_NAME.into(),
            serde_json::Value::String("Alice".into()),
        );

        let mut server_copy = incoming("s1", vec![]);
        server_copy.prompted_by_name = Some("Someone Else".into());

        let merged = merge_pulled(Some(local), &server_copy, &origin());
        assert_eq!(
            merged.metadata[lineage_git::PROMPTED_BY_NAME],
            serde_json::Value::String("Alice".into())
        );
    }

    #[test]
    fn a_display_field_the_local_copy_lacks_is_folded_into_metadata() {
        let mut server_copy = incoming("s1", vec![]);
        server_copy.prompted_by_name = Some("Alice".into());
        server_copy.model = Some("claude-sonnet".into());

        let merged = merge_pulled(None, &server_copy, &origin());
        assert_eq!(
            merged.metadata[lineage_git::PROMPTED_BY_NAME],
            serde_json::Value::String("Alice".into())
        );
        assert_eq!(
            merged.metadata["model"],
            serde_json::Value::String("claude-sonnet".into())
        );
    }

    #[test]
    fn an_unknown_role_reads_as_system_rather_than_failing_the_pull() {
        assert!(matches!(role_of("assistant"), Role::Assistant));
        assert!(matches!(role_of("Tool"), Role::Tool));
        assert!(matches!(role_of("narrator"), Role::System));
    }
}
