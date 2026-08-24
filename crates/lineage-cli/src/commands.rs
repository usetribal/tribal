use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use lineage_adapters::all_adapters;
use lineage_core::{
    conversation_modified_code, derive_session_id, display_title, generate_architecture_summary,
    humanize_text, id_prefix, AgentKind, CommitMappingMode, LastImportState, LineageId,
    LineageRepoConfig, SOURCE_MTIME_KEY,
};
use lineage_git::{
    assemble_batch, best_commit_for_conversation, blame_with_lineage, chunk_batch, delete_session,
    ensure_gitattributes, hydrate_media_artifacts, lfs_fetch, lfs_push, lfs_status,
    link_session_to_commit, list_session_ids, map_commit_to_sessions,
    materialize_session_at_commit, open_repo, persist_import, purge_orphans, read_conversation,
    read_conversation_stored, read_repo_config, remap_orphaned_commits, resolve_session,
    stamp_prompted_by, sync_push_with_progress, write_last_import, write_repo_config, LineageRepo,
    PROMPTED_BY_EMAIL, PROMPTED_BY_NAME, SYNC_CONVERSATIONS_PER_CHUNK,
};
use lineage_policy::{
    apply_policy, is_private_session, policy_from_repo_config, prepare_for_export, PolicyConfig,
};
use lineage_search::{LineageIndex, SearchHit};

use crate::auth;
use crate::events::{EventLog, Outcome};
use crate::migrate;
use crate::ui::{self, ScanRow};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn init_config(repo_path: &Path) -> Result<()> {
    init_config_impl(repo_path, true)
}

pub(crate) fn init_config_quiet(repo_path: &Path) -> Result<()> {
    init_config_impl(repo_path, false)
}

fn init_config_impl(repo_path: &Path, verbose: bool) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let config = LineageRepoConfig::default();
    write_repo_config(repo.inner(), &config)?;
    ensure_gitattributes(repo.inner())?;
    if verbose {
        ui::action("wrote default config to refs/lineage/config");
        ui::action("ensured .gitattributes for .lineage/media/** LFS pointers");
    }
    Ok(())
}

pub fn import(
    repo_path: &Path,
    agents: &[String],
    since: Option<&str>,
    link_head: bool,
    incremental: bool,
) -> Result<()> {
    let lineage_repo = open_repo(repo_path)?;
    let inner = lineage_repo.inner();
    let workdir = lineage_repo.workdir();

    let repo_config = read_repo_config(inner)?;
    let policy = policy_from_repo_config(&repo_config);
    let since_dt = since.and_then(parse_since);

    let filter_all = agents.is_empty() || agents.iter().any(|a| a == "all");
    let wanted: Vec<AgentKind> = if filter_all {
        vec![AgentKind::Cursor, AgentKind::Claude, AgentKind::Codex]
    } else {
        agents.iter().filter_map(|a| AgentKind::parse(a)).collect()
    };

    // The transcript mtime each stored session was last read at, keyed by id.
    // A file untouched since then has nothing new, so it never has to be parsed.
    let stored_mtimes: std::collections::HashMap<LineageId, DateTime<Utc>> = if incremental {
        let mut mtimes = std::collections::HashMap::new();
        for id in list_session_ids(inner)? {
            let Some(conv) = read_conversation_stored(inner, &id)? else {
                continue;
            };
            let stamped = conv
                .metadata
                .get(SOURCE_MTIME_KEY)
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            if let Some(mtime) = stamped {
                mtimes.insert(id, mtime);
            }
        }
        mtimes
    } else {
        std::collections::HashMap::new()
    };

    let adapters = all_adapters(workdir);
    let mut conversations = Vec::new();
    let mut errors = 0usize;
    let mut skipped = 0usize;
    let mut discovered = std::collections::BTreeMap::new();

    for (kind, adapter) in adapters {
        if !wanted.contains(&kind) {
            continue;
        }
        let sessions = adapter.discover()?;
        discovered.insert(kind.as_str().to_string(), sessions.len());
        ui::action(format!(
            "discovered {} {} session(s)",
            sessions.len(),
            kind.as_str()
        ));
        for session in sessions {
            if let Some(since_dt) = since_dt {
                if session.started_at.map(|t| t < since_dt).unwrap_or(false) {
                    skipped += 1;
                    continue;
                }
            }

            let source_mtime = fs::metadata(&session.source_path)
                .and_then(|meta| meta.modified())
                .map(DateTime::<Utc>::from)
                .ok();

            // Skip a stored session whose transcript has not been written since
            // it was last read. Direct lookup, not a substring scan over stored
            // ids: the id is a hash, so a token never appears inside one, and
            // the old `contains` check could never match — which is why every
            // session was re-imported on every commit.
            if incremental {
                let id = derive_session_id(kind, session.session_token());
                if let (Some(modified), Some(read_at)) = (source_mtime, stored_mtimes.get(&id)) {
                    if modified <= *read_at {
                        skipped += 1;
                        continue;
                    }
                }
            }

            match adapter.read(&session) {
                Ok(mut conv) => {
                    let source = session.source_path.display().to_string();
                    conv.metadata
                        .insert("source".into(), serde_json::Value::String(source.clone()));
                    // Stamped from the mtime read before parsing, not re-statted
                    // after: a vendor appending mid-import would otherwise stamp
                    // a time newer than the content actually read, and the next
                    // run would skip turns it never stored.
                    if let Some(modified) = source_mtime {
                        conv.metadata.insert(
                            SOURCE_MTIME_KEY.into(),
                            serde_json::Value::String(modified.to_rfc3339()),
                        );
                    }
                    if is_private_session(&source, &repo_config) {
                        conv.private = true;
                    }
                    if repo_config.import_only_code_sessions && !conversation_modified_code(&conv) {
                        skipped += 1;
                        continue;
                    }
                    stamp_prompted_by(inner, &mut conv)?;
                    let summary = generate_architecture_summary(&conv);
                    conv.metadata.insert(
                        "architecture_summary".into(),
                        serde_json::Value::String(summary),
                    );
                    let policy_result = apply_policy(&policy, conv);
                    conversations.push(policy_result.conversation);
                }
                Err(e) => {
                    eprintln!("  skip {}: {e}", session.source_path.display());
                    errors += 1;
                }
            }
        }
    }

    if link_head {
        for c in &mut conversations {
            let sha = match repo_config.commit_mapping {
                CommitMappingMode::None => continue,
                CommitMappingMode::Head => inner
                    .head()
                    .ok()
                    .and_then(|h| h.peel_to_commit().ok())
                    .map(|c| c.id().to_string()),
                CommitMappingMode::Auto => best_commit_for_conversation(inner, c)
                    .ok()
                    .flatten()
                    .map(|m| {
                        c.metadata
                            .insert("commit_match_score".into(), serde_json::json!(m.score));
                        c.metadata
                            .insert("commit_match_signals".into(), serde_json::json!(m.signals));
                        m.commit_sha
                    })
                    .or_else(|| {
                        inner
                            .head()
                            .ok()
                            .and_then(|h| h.peel_to_commit().ok())
                            .map(|c| c.id().to_string())
                    }),
            };
            if let Some(sha) = sha {
                if !c.commit_shas.contains(&sha) {
                    c.commit_shas.push(sha);
                }
            }
        }
    }

    let results = persist_import(inner, &conversations)?;
    let imported_ids: Vec<LineageId> = conversations.iter().map(|c| c.id.clone()).collect();
    index_persisted_sessions(&lineage_repo, &imported_ids)?;
    write_last_import(inner, &LastImportState::new(imported_ids))?;

    let line_objects: usize = results.iter().map(|r| r.line_objects_written).sum();
    ui::action(format!(
        "imported {} session(s), {} line object(s), {} skipped, {} error(s)",
        results.len(),
        line_objects,
        skipped,
        errors
    ));
    EventLog::for_git_dir(&lineage_repo.git_dir()).append(
        Utc::now(),
        "import",
        Outcome::Ok,
        serde_json::json!({
            "agents": wanted.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
            "discovered": discovered,
            "imported": results.len(),
            "skipped": skipped,
            "errors": errors,
            "session_ids": results.iter().map(|r| r.session_id.as_str()).collect::<Vec<_>>(),
            "line_objects_written": line_objects,
        }),
    );
    for r in results {
        ui::row(
            &r.session_id,
            format!("{} line objects", r.line_objects_written),
        );
    }
    Ok(())
}

fn parse_since(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc()))
        })
}

fn is_false_bool(value: &bool) -> bool {
    !*value
}

fn vendor_session_id(conv: &lineage_core::Conversation) -> Option<String> {
    let keys = ["cursor_session_id", "claude_session_id", "codex_session_id"];
    for key in keys {
        if let Some(value) = conv.metadata.get(key).and_then(|v| v.as_str()) {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

#[derive(serde::Serialize)]
pub struct SessionSummary {
    id: String,
    title: String,
    agent: String,
    turns: usize,
    started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    models_used: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "is_false_bool")]
    is_sidechain: bool,
    /// A fork also has a parent, so it must be reported separately or a
    /// consumer reading `is_sidechain` would call it a harness-spawned branch.
    #[serde(skip_serializing_if = "is_false_bool")]
    is_fork: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    vendor_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompted_by_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompted_by_name: Option<String>,
}

#[derive(serde::Serialize)]
struct CommitSessionSummary {
    commit_sha: String,
    session_ids: Vec<String>,
}

pub fn list(repo_path: &Path, commit: Option<&str>, json: bool) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let inner = repo.inner();

    if let Some(sha) = commit {
        let ids = map_commit_to_sessions(inner, sha)?;
        if json {
            let summary = CommitSessionSummary {
                commit_sha: sha.to_string(),
                session_ids: ids.iter().map(|id| id.to_string()).collect(),
            };
            ui::json(&summary)?;
        } else {
            let mut summaries = collect_summaries_from_ids(inner, ids)?;
            summaries.sort_by(|a, b| b.started_at.cmp(&a.started_at));
            print_session_list(&summaries, "no sessions linked to this commit");
        }
        return Ok(());
    }

    let ids = list_session_ids(inner)?;
    let mut summaries = collect_summaries_from_ids(inner, ids)?;

    // Newest first. `list_session_ids` returns ref order, which is neither
    // chronological nor stable across machines, so a user scanning for "the
    // session I ran this morning" had to read every row.
    summaries.sort_by(|a, b| b.started_at.cmp(&a.started_at));

    if json {
        ui::json(&summaries)?;
    } else {
        print_session_list(&summaries, "no sessions — run tribal import");
    }
    Ok(())
}

fn print_session_list(summaries: &[SessionSummary], empty: &str) {
    if summaries.is_empty() {
        ui::empty(empty);
        return;
    }
    let rows: Vec<ScanRow> = summaries.iter().map(ScanRow::from).collect();
    ui::print_scan_rows(&rows);
}

pub fn collect_session_summaries(repo_path: &Path) -> Result<Vec<SessionSummary>> {
    let repo = open_repo(repo_path)?;
    let ids = list_session_ids(repo.inner())?;
    let mut summaries = collect_summaries_from_ids(repo.inner(), ids)?;
    summaries.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(summaries)
}

fn collect_summaries_from_ids(
    inner: &git2::Repository,
    ids: Vec<LineageId>,
) -> Result<Vec<SessionSummary>> {
    let mut summaries = Vec::new();
    for id in ids {
        if let Some(conv) = read_conversation(inner, &id)? {
            summaries.push(summary_from_conversation(&conv));
        }
    }
    Ok(summaries)
}

fn summary_from_conversation(conv: &lineage_core::Conversation) -> SessionSummary {
    SessionSummary {
        id: conv.id.to_string(),
        title: display_title(conv),
        agent: conv.agent.as_str().to_string(),
        turns: conv.turns.len(),
        started_at: conv.started_at.to_rfc3339(),
        model: conv.primary_model(),
        models_used: conv.models_used(),
        git_branch: conv
            .metadata
            .get("git_branch")
            .and_then(|v| v.as_str())
            .map(String::from),
        parent_session_id: conv.parent_session_id.as_ref().map(|id| id.to_string()),
        is_sidechain: conv
            .metadata
            .get("is_sidechain")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || (conv.parent_session_id.is_some() && !conv.is_fork()),
        is_fork: conv.is_fork(),
        vendor_session_id: vendor_session_id(conv),
        prompted_by_email: conv
            .metadata
            .get(PROMPTED_BY_EMAIL)
            .and_then(|v| v.as_str())
            .map(String::from),
        prompted_by_name: conv
            .metadata
            .get(PROMPTED_BY_NAME)
            .and_then(|v| v.as_str())
            .map(String::from),
    }
}

impl From<&SessionSummary> for ScanRow {
    fn from(s: &SessionSummary) -> Self {
        ScanRow {
            title: s.title.clone(),
            id: id_prefix(&s.id),
            turns: s.turns,
            day: ui::day(&s.started_at).to_string(),
            agent: s.agent.clone(),
            model: s.model.clone().unwrap_or_else(|| "—".into()),
            who: s
                .prompted_by_name
                .clone()
                .or_else(|| s.prompted_by_email.clone()),
            tag: s.is_fork.then(|| "(fork)".into()),
        }
    }
}

/// One row a person can choose from. The title is what makes a list of 100
/// sessions navigable; the id stays visible for copy/paste and fork hints.
/// Test helper: production prints through `list_rows` so columns share widths.
#[cfg(test)]
pub(crate) fn list_row(s: &SessionSummary) -> String {
    ui::format_scan_row(&ScanRow::from(s))
}

/// Format a batch of rows so their columns line up against each other.
///
/// No caller in the binary any more: the interactive picker it used to feed
/// (`inquire`, a flat list of pre-rendered labels) is now the `lineage-select`
/// TUI, which lays out its own rows. Kept because the shared-column behaviour is
/// worth a test of its own, and a future batch formatter wants exactly this.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn list_rows(summaries: &[SessionSummary]) -> Vec<String> {
    ui::format_scan_rows(&summaries.iter().map(ScanRow::from).collect::<Vec<_>>())
}

pub fn show(repo_path: &Path, session_hint: &str, json: bool, hydrate_images: bool) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let id = resolve_session(repo.inner(), session_hint).map_err(|error| error.to_string())?;
    let mut conv = read_conversation(repo.inner(), &id)?
        .ok_or_else(|| format!("session not found: {session_hint}"))?;

    if hydrate_images {
        let _ = hydrate_media_artifacts(repo.inner(), &mut conv)?;
    }

    if json {
        ui::raw_line(conv.to_json()?);
    } else {
        print_session(&conv);
    }
    Ok(())
}

fn print_session(conv: &lineage_core::Conversation) {
    ui::heading(&display_title(conv));
    ui::kv("Id", &conv.id);
    ui::kv("Agent", conv.agent.as_str());
    ui::kv("Started", ui::human_date(conv.started_at));
    ui::kv("Turns", conv.turns.len());
    if conv.private {
        ui::kv("Private", "yes");
    }
    if let Some(origin) = &conv.fork_origin {
        ui::kv(
            "Forked",
            format!(
                "from {} on {}",
                origin.source_session_id,
                origin.forked_at.format("%-d %b %Y")
            ),
        );
    }
    if let Some(model) = conv.primary_model() {
        ui::kv("Model", model);
    }
    let author = conv
        .metadata
        .get(PROMPTED_BY_NAME)
        .and_then(|v| v.as_str())
        .or_else(|| {
            conv.metadata
                .get(PROMPTED_BY_EMAIL)
                .and_then(|v| v.as_str())
        });
    if let Some(author) = author {
        ui::kv("Author", author);
    }
    let models = conv.models_used();
    if models.len() > 1 {
        ui::kv("Models", models.join(", "));
    }
    for (key, label) in [
        ("git_branch", "Branch"),
        ("claude_code_version", "Claude"),
        ("codex_cli_version", "Codex"),
        ("codex_originator", "Origin"),
        ("cursor_session_id", "Vendor id"),
        ("claude_session_id", "Vendor id"),
        ("codex_session_id", "Vendor id"),
    ] {
        if let Some(value) = conv.metadata.get(key).and_then(|v| v.as_str()) {
            ui::kv(label, value);
        }
    }
    if let Some(summary) = conv
        .metadata
        .get("session_summary")
        .and_then(|v| v.as_str())
        .or_else(|| {
            conv.metadata
                .get("architecture_summary")
                .and_then(|v| v.as_str())
        })
    {
        ui::blank();
        ui::section("Summary");
        for line in summary.lines() {
            ui::indent(line);
        }
    }
    if conv.turns.is_empty() {
        return;
    }
    ui::blank();
    ui::section("Turns");
    for (i, turn) in conv.turns.iter().enumerate() {
        let preview = turn_preview(turn);
        ui::turn(i, turn.role, turn.model.as_deref(), &preview);
        if !turn.tool_calls.is_empty() {
            ui::indent(format!("        {} tools", turn.tool_calls.len()));
        }
    }
}

fn turn_preview(turn: &lineage_core::Turn) -> String {
    let cleaned = humanize_text(&turn.content);
    if cleaned.is_empty() && !turn.tool_calls.is_empty() {
        return turn
            .tool_calls
            .iter()
            .map(|call| call.name.as_str())
            .take(3)
            .collect::<Vec<_>>()
            .join(", ");
    }
    cleaned.chars().take(120).collect()
}

pub fn blame(repo_path: &Path, target: &str, json: bool) -> Result<()> {
    let (path, line) = parse_blame_target(target)?;
    let repo = open_repo(repo_path)?;
    let rel = std::path::Path::new(&path);

    let result = blame_with_lineage(repo.inner(), rel, line)?;

    if json {
        ui::json(&result)?;
        return Ok(());
    }

    ui::heading(&format!("{}:{}", path, result.line));
    ui::kv("Commit", &result.commit_sha);
    ui::kv("Sessions", result.sessions.len());
    for id in &result.sessions {
        ui::indent(ui::accent(id.as_str()));
    }
    if !result.line_objects.is_empty() {
        ui::blank();
        ui::section("Line objects");
        for obj in &result.line_objects {
            ui::row(
                format!("{}:{}", obj.line_range[0], obj.line_range[1]),
                format!(
                    "turn {}  ({})",
                    obj.turn_id,
                    ui::confidence_label(obj.confidence)
                ),
            );
        }
    }
    if !result.matches.is_empty() {
        ui::blank();
        ui::section("Matches");
        for m in &result.matches {
            let range = m
                .line_range
                .map(|r| format!("{}:{}", r[0], r[1]))
                .unwrap_or_else(|| "?".into());
            ui::indent(format!(
                "{}  lines {range}  ({})  {}",
                ui::accent(format!("turn {}", m.turn_id)),
                ui::confidence_label(m.confidence),
                m.content_preview
            ));
        }
    }
    Ok(())
}

pub fn export(repo_path: &Path, redact: bool, format: &str) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let inner = repo.inner();
    let ids = list_session_ids(inner)?;

    let repo_config = read_repo_config(inner)?;
    let mut policy = policy_from_repo_config(&repo_config);
    if redact {
        policy.strip_private = true;
        policy.redaction_rules = PolicyConfig::default_safe().redaction_rules;
    }

    let mut out = Vec::new();
    for id in ids {
        if let Some(conv) = read_conversation(inner, &id)? {
            if policy.strip_private && conv.private {
                continue;
            }
            out.push(prepare_for_export(&policy, conv));
        }
    }

    match format {
        "json" => ui::json(&out)?,
        "jsonl" => {
            for conv in out {
                ui::jsonl(&conv)?;
            }
        }
        other => return Err(format!("unsupported format: {other}").into()),
    }
    Ok(())
}

/// Prints an approval prompt and waits, polling `poll` until it returns a value.
///
/// Both approvals in a login share this shape — same deadline, same backoff, same
/// expiry message — so they share the loop; only the prompt and the poll differ.
fn await_approval<T>(
    prompt: &str,
    url: &str,
    entry_url: &str,
    user_code: &str,
    expires_in: u64,
    interval: u64,
    mut poll: impl FnMut() -> Result<Option<PollStep<T>>>,
) -> Result<T> {
    ui::action(prompt);
    ui::hero(url);
    // A prefilled URL can still fail (a browser that drops the query, a code the
    // page asks to retype), so the bare entry point stays visible as a fallback.
    if entry_url == url {
        ui::action(format!("and confirm code {user_code}."));
    } else {
        ui::action(format!(
            "and confirm code {user_code} (or enter it at {entry_url})."
        ));
    }
    ui::action("Waiting for approval…");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(expires_in);
    let mut interval = interval;
    loop {
        match poll()? {
            Some(PollStep::Done(value)) => return Ok(value),
            // OAuth device-flow rule: back off by 5s when told to slow down.
            Some(PollStep::SlowDown) => interval += 5,
            None => {}
        }
        if std::time::Instant::now() >= deadline {
            return Err("approval request expired before it was granted: run login again".into());
        }
        std::thread::sleep(std::time::Duration::from_secs(interval));
    }
}

enum PollStep<T> {
    Done(T),
    SlowDown,
}

/// Runs the device login against a Tribal server: prints the verification URL
/// and code, polls until the browser approval completes the login server-side,
/// and stores the returned session handle (the durable credential — the JWT it
/// mints is short-lived and re-minted per command).
///
/// A first-time user needs a second approval. The identity provider's device flow
/// cannot request the organization scope that membership derivation needs, so the
/// server asks for a separate grant against the forge and the login continues
/// through it. Returning users never see this step.
pub fn login(server: Option<&str>) -> Result<()> {
    let server = auth::resolve_server(server)?;
    let start = auth::device_start(&server)?;

    let outcome = await_approval(
        "To sign in, open this URL in a browser:",
        &start.verification_uri_complete,
        &start.verification_uri,
        &start.user_code,
        start.expires_in,
        start.interval,
        || match auth::device_poll(&server, &start.device_code)? {
            auth::DevicePollResponse::Pending { slow_down } => {
                Ok(slow_down.then_some(PollStep::SlowDown))
            }
            auth::DevicePollResponse::TrustGrantRequired { trust_grant_handle } => Ok(Some(
                PollStep::Done(LoginOutcome::TrustGrant(trust_grant_handle)),
            )),
            auth::DevicePollResponse::Complete { session_handle, .. } => {
                Ok(Some(PollStep::Done(LoginOutcome::Session(session_handle))))
            }
        },
    )?;

    let session_handle = match outcome {
        LoginOutcome::Session(handle) => handle,
        LoginOutcome::TrustGrant(handle) => {
            grant_organization_access(&server, &handle)?;
            handle
        }
    };

    auth::store_login(&server, session_handle)?;
    ui::action(format!(
        "Logged in. Credentials stored in {}.",
        auth::credentials_path()?.display()
    ));
    Ok(())
}

enum LoginOutcome {
    Session(String),
    TrustGrant(String),
}

/// Second approval of a first-time login: grants the server a one-off read of the
/// user's organizations so it can derive their workspaces. The server discards
/// that access once derivation is done.
fn grant_organization_access(server: &str, trust_grant_handle: &str) -> Result<()> {
    let start = auth::trust_grant_start(server)?;

    ui::blank();
    let tenant_count = await_approval(
        "One more step. To find your organizations, open this URL:",
        &start.verification_uri,
        &start.verification_uri,
        &start.user_code,
        start.expires_in,
        start.interval,
        || match auth::trust_grant_poll(server, trust_grant_handle, &start.device_code)? {
            auth::TrustGrantPollResponse::Pending { slow_down } => {
                Ok(slow_down.then_some(PollStep::SlowDown))
            }
            auth::TrustGrantPollResponse::Complete { tenant_count } => {
                Ok(Some(PollStep::Done(tenant_count)))
            }
        },
    )?;

    ui::action(format!(
        "Found {tenant_count} workspace{}.",
        if tenant_count == 1 { "" } else { "s" }
    ));
    Ok(())
}

pub fn sync(
    repo_path: &Path,
    server: Option<&str>,
    token: Option<&str>,
    remote: &str,
) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let inner = repo.inner();
    let server = auth::resolve_server(server)?;
    let token = auth::resolve_token(&server, token)?;

    // Redact and drop private sessions before anything crosses the wire — the
    // same guarantee the export path provides (sync-protocol-v0 "Privacy").
    let repo_config = read_repo_config(inner)?;
    let mut policy = policy_from_repo_config(&repo_config);
    policy.strip_private = true;
    policy.redaction_rules = PolicyConfig::default_safe().redaction_rules;

    let mut conversations = Vec::new();
    for id in list_session_ids(inner)? {
        if let Some(conv) = read_conversation_stored(inner, &id)? {
            if conv.private {
                continue;
            }
            conversations.push(prepare_for_export(&policy, conv));
        }
    }

    let batch = assemble_batch(inner, remote, conversations)?;
    let chunk_count = chunk_batch(&batch, SYNC_CONVERSATIONS_PER_CHUNK).len();
    ui::action(format!(
        "syncing {} conversation(s), {} line object(s), {} commit link(s), {} blob(s) to {server} in {chunk_count} chunk(s)",
        batch.conversations.len(),
        batch.line_objects.len(),
        batch.session_commit_links.len(),
        batch.blobs.len()
    ));

    let outcome = sync_push_with_progress(inner, &server, &token, &batch, |done, total| {
        if total > 1 {
            ui::indent(format!("chunk {done}/{total}"));
        }
    })?;
    let report = &outcome.report;
    ui::action(format!(
        "synced to repo {} ({} accepted, {} noop, {} rejected, {} pending, {} blob(s) uploaded)",
        report.repo_id,
        report.accepted,
        report.noop,
        report.rejected,
        report.pending,
        report.blobs_uploaded
    ));
    EventLog::for_git_dir(&repo.git_dir()).append(
        Utc::now(),
        "sync",
        // A rejected object is a failed sync from the user's point of view
        // (the command exits nonzero below), and the event should agree.
        if report.rejected > 0 {
            Outcome::Error
        } else {
            Outcome::Ok
        },
        serde_json::json!({
            "server": server,
            "remote": remote,
            "batch": {
                "conversations": batch.conversations.len(),
                "line_objects": batch.line_objects.len(),
                "session_commit_links": batch.session_commit_links.len(),
                "blobs": batch.blobs.len(),
                "chunks": report.chunks,
            },
            "blobs_uploaded": report.blobs_uploaded,
            "response": serde_json::to_value(&outcome.response).unwrap_or_default(),
        }),
    );
    if report.rejected > 0 {
        return Err(format!("{} object(s) rejected by server", report.rejected).into());
    }
    Ok(())
}

pub fn search(repo_path: &Path, query: &str) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
    let hits: Vec<SearchHit> = index.search(query, 20)?;
    if hits.is_empty() {
        let _ = index.rebuild(repo.inner());
        let hits = index.search(query, 20)?;
        if hits.is_empty() {
            ui::empty(&format!("no results for '{query}'"));
            return Ok(());
        }
        print_hits(repo.inner(), &hits);
        return Ok(());
    }
    print_hits(repo.inner(), &hits);
    Ok(())
}

fn print_hits(inner: &git2::Repository, hits: &[SearchHit]) {
    ui::heading(&format!("{} hit(s)", hits.len()));
    for hit in hits {
        let title = read_conversation(inner, &LineageId::from(hit.session_id.as_str()))
            .ok()
            .flatten()
            .map(|conv| display_title(&conv))
            .unwrap_or_else(|| hit.session_id.clone());
        ui::indent(format!(
            "{}  {}  {}  {}",
            ui::accent(&title),
            ui::dim(&hit.session_id),
            ui::dim(format!("score={:.2}", hit.score)),
            hit.snippet
        ));
    }
}

/// Add freshly persisted sessions to the search index.
///
/// Persisting writes refs; indexing is a separate stage downstream of it
/// (`docs/ARCHITECTURE.md`, import flow steps 5 and 8), and `lineage-git` cannot
/// reach the index without inverting the crate graph. The CLI is the one crate
/// that depends on both, so this is where the two meet — shared by every path
/// that persists rather than repeated at each, because a path that forgets it
/// writes sessions `search` and `context query` cannot see.
pub fn index_persisted_sessions(repo: &LineageRepo, session_ids: &[LineageId]) -> Result<()> {
    if session_ids.is_empty() {
        return Ok(());
    }

    let inner = repo.inner();
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
    for id in session_ids {
        if let Some(conv) = read_conversation(inner, id)? {
            index.index_conversation(&conv)?;
        }
    }
    // Mirror the sessions' line objects and chain them, so `context chain` is
    // answerable without a full rebuild.
    index.populate_line_tables_for_sessions(inner, session_ids)?;
    Ok(())
}

/// Index sessions someone else's machine produced, without letting the index
/// decide whether the operation succeeded.
///
/// `fork` and `pull` have already written the session to refs by the time this
/// runs, so a failure here costs searchability and nothing else — reporting it
/// as a failed fork would tell the receiver nothing landed when everything did.
/// The warning names the recovery, because a silently unsearchable session is
/// the worse outcome of the two.
pub fn index_persisted_sessions_best_effort(repo: &LineageRepo, session_ids: &[LineageId]) {
    if let Err(error) = index_persisted_sessions(repo, session_ids) {
        eprintln!(
            "warning: session saved but not added to the search index ({error}); \
             run `tribal rebuild index` to make it searchable"
        );
    }
}

/// Rebuild only the lexical search index; embeddings are a separate pass
/// (`rebuild embeddings`) so a plain index rebuild stays cheap. `embed` opts
/// into running that pass afterwards, same as `rebuild --embed`.
pub fn rebuild_index(repo_path: &Path, embed: bool) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let inner = repo.inner();
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;

    let mut bar = crate::progress::SessionProgress::new("indexing");
    let indexed = index.rebuild_with_progress(inner, &mut |done, total| bar.update(done, total))?;
    bar.finish();

    let mut lines_bar = crate::progress::SessionProgress::new("chaining");
    index.populate_line_tables(inner, &mut |done, total| lines_bar.update(done, total))?;
    lines_bar.finish();

    let embedded = if embed { run_embed_pass(repo_path)? } else { 0 };

    if embedded > 0 {
        ui::action(format!("index rebuilt ({embedded} session(s) embedded)"));
    } else {
        ui::action("index rebuilt");
    }
    EventLog::for_git_dir(&repo.git_dir()).append(
        Utc::now(),
        "rebuild_index",
        Outcome::Ok,
        serde_json::json!({ "sessions_indexed": indexed, "sessions_embedded": embedded }),
    );
    Ok(())
}

/// Run the dense-embedding backfill with a per-session progress bar, returning
/// the number of sessions embedded.
fn run_embed_pass(repo_path: &Path) -> Result<usize> {
    let mut bar = crate::progress::SessionProgress::new("embedding");
    let embedded = crate::retrieval_cmd::embed_all_sessions(repo_path, &mut |done, remaining| {
        bar.update(done, remaining)
    })?;
    bar.finish();
    Ok(embedded)
}

/// `tribal rebuild embeddings`: the dense-embedding backfill on its own,
/// symmetric with `rebuild index`.
pub fn rebuild_embeddings(repo_path: &Path) -> Result<()> {
    let embedded = run_embed_pass(repo_path)?;
    if embedded > 0 {
        ui::action(format!(
            "embeddings rebuilt ({embedded} session(s) embedded)"
        ));
    } else {
        ui::action("embeddings up to date (0 session(s) embedded)");
    }
    let repo = open_repo(repo_path)?;
    EventLog::for_git_dir(&repo.git_dir()).append(
        Utc::now(),
        "rebuild_embeddings",
        Outcome::Ok,
        serde_json::json!({ "sessions_embedded": embedded }),
    );
    Ok(())
}

pub fn link(repo_path: &Path, session_id: &str, commit_sha: &str) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let id = LineageId::from(session_id);
    let lines = link_session_to_commit(repo.inner(), &id, commit_sha)?;
    // The link is written to the conversation ref, not through
    // `index_conversation`, so the session↔commit mirror is maintained here or
    // it goes stale until the next rebuild.
    LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?
        .link_session_commit(session_id, commit_sha)?;
    ui::action(format!(
        "linked {session_id} -> {commit_sha} ({lines} line object(s))"
    ));
    EventLog::for_git_dir(&repo.git_dir()).append(
        Utc::now(),
        "link",
        Outcome::Ok,
        serde_json::json!({
            "commit_sha": commit_sha,
            "sessions": [{ "session_id": session_id, "line_objects": lines }],
            "trigger": "manual",
        }),
    );
    Ok(())
}

pub fn materialize(repo_path: &Path, commit: Option<&str>, session: Option<&str>) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let inner = repo.inner();

    let commit_sha = if let Some(sha) = commit {
        sha.to_string()
    } else {
        inner
            .head()
            .map_err(|e| format!("{e}"))?
            .peel_to_commit()
            .map_err(|e| format!("{e}"))?
            .id()
            .to_string()
    };

    let session_ids: Vec<LineageId> = if let Some(id) = session {
        vec![LineageId::from(id)]
    } else {
        list_session_ids(inner)?
    };

    let mut total = 0usize;
    let mut sessions = Vec::new();
    for id in session_ids {
        let count = materialize_session_at_commit(inner, &id, &commit_sha)?;
        ui::row(
            &id,
            format!("materialized {count} line object(s) @ {commit_sha}"),
        );
        sessions.push(serde_json::json!({ "session_id": id.as_str(), "line_objects": count }));
        total += count;
    }
    ui::action(format!("done ({total} line object(s))"));
    EventLog::for_git_dir(&repo.git_dir()).append(
        Utc::now(),
        "materialize",
        Outcome::Ok,
        serde_json::json!({ "commit_sha": commit_sha, "sessions": sessions }),
    );
    Ok(())
}

pub fn remap(repo_path: &Path) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let report = remap_orphaned_commits(repo.inner())?;
    ui::action(format!(
        "remapped {} orphan commit(s) ({} patch-id match(es)), {} session(s), {} line object(s)",
        report.remapped_commits,
        report.patch_id_matches,
        report.rematerialized_sessions,
        report.line_objects_updated
    ));
    Ok(())
}

pub fn lfs_status_cmd(repo_path: &Path) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let report = lfs_status(repo.inner())?;
    ui::heading("LFS status");
    ui::kv_width("referenced", report.referenced, 14);
    ui::kv_width("present local", report.present_local, 14);
    ui::kv_width("transport refs", report.transport_refs, 14);
    ui::kv_width("git-lfs CLI", report.git_lfs_available, 14);
    if !report.missing_local.is_empty() {
        ui::kv_width("missing local", report.missing_local.len(), 14);
        for b in report.missing_local.iter().take(10) {
            ui::indent(format!("- {}", ui::dim(b)));
        }
    }
    Ok(())
}

pub fn lfs_push_cmd(repo_path: &Path, remote: Option<&str>) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let remote = remote.unwrap_or("origin");
    let report = lfs_push(repo.inner(), remote)?;
    ui::action(format!(
        "pushed LFS objects to {remote} via {} ({} uploaded, {} skipped)",
        report.method, report.uploaded, report.skipped
    ));
    Ok(())
}

pub fn delete_session_cmd(repo_path: &Path, session_id: &str, purge_blobs: bool) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let id = LineageId::from(session_id);
    let report = delete_session(repo.inner(), &id, purge_blobs)?;
    ui::action(format!(
        "deleted session {} ({} note(s) updated, {} line object(s) removed, {} blob(s) purged)",
        report.session_id, report.notes_updated, report.line_objects_deleted, report.blobs_purged
    ));
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
    let _ = index.rebuild(repo.inner());
    Ok(())
}

pub fn gc_cmd(repo_path: &Path) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let report = purge_orphans(repo.inner())?;
    ui::action(format!(
        "purged {} orphan line object(s), {} unreferenced blob(s), {} transport ref(s)",
        report.line_objects_purged, report.blobs_purged, report.transport_refs_purged
    ));
    Ok(())
}

pub fn lfs_fetch_cmd(repo_path: &Path, remote: Option<&str>) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let remote = remote.unwrap_or("origin");
    let report = lfs_fetch(repo.inner(), remote)?;
    ui::action(format!(
        "fetched LFS objects from {remote} via {} ({} downloaded, {} skipped)",
        report.method, report.downloaded, report.skipped
    ));
    Ok(())
}

fn parse_blame_target(target: &str) -> Result<(String, u32)> {
    if let Some((path, line_str)) = target.rsplit_once(':') {
        if let Ok(line) = line_str.parse::<u32>() {
            if path.contains('/') || path.contains('.') {
                return Ok((path.to_string(), line));
            }
        }
    }
    Ok((target.to_string(), 1))
}

/// Rebuild the whole derived layer (notes, line objects, search index) from
/// stored conversations × git history under current code, then replay manual
/// links from the event log — they are user intent, not derivable, so the
/// wipe must not lose them. Pre-event-log manual links cannot be identified
/// and are dropped (reported).
pub fn rebuild(repo_path: &Path, embed: bool) -> Result<()> {
    let repo = open_repo(repo_path)?;

    // The commit scan is the slow phase of a full rebuild (a revwalk over all
    // history); its length is unknown up front, so it gets a running-count
    // spinner rather than a ratio bar.
    let scan = crate::progress::Spinner::new("scanning commits");
    let report =
        lineage_git::rebuild_links_with_progress(repo.inner(), &mut |scanned| scan.set(scanned))?;
    scan.finish();

    let log = EventLog::for_git_dir(&repo.git_dir());
    for (sha, link) in &report.linked_commits {
        let sessions: Vec<serde_json::Value> = link
            .linked
            .iter()
            .map(|s| {
                serde_json::json!({
                    "session_id": s.session_id.as_str(),
                    "line_objects": s.line_objects,
                    "basis": s.basis.as_str(),
                })
            })
            .collect();
        log.append(
            Utc::now(),
            "link",
            Outcome::Ok,
            serde_json::json!({
                "commit_sha": sha,
                "sessions": sessions,
                "trigger": "rebuild",
            }),
        );
    }

    let mut manual_replayed = 0usize;
    for event in log.read_entries() {
        if event["op"] != "link" || event["detail"]["trigger"] != "manual" {
            continue;
        }
        let Some(commit_sha) = event["detail"]["commit_sha"].as_str() else {
            continue;
        };
        for session in event["detail"]["sessions"].as_array().into_iter().flatten() {
            let Some(session_id) = session["session_id"].as_str() else {
                continue;
            };
            let id = LineageId::from(session_id);
            if link_session_to_commit(repo.inner(), &id, commit_sha).is_ok() {
                manual_replayed += 1;
            }
        }
    }

    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
    let mut bar = crate::progress::SessionProgress::new("indexing");
    let sessions_indexed =
        index.rebuild_with_progress(repo.inner(), &mut |done, total| bar.update(done, total))?;
    bar.finish();

    let mut lines_bar = crate::progress::SessionProgress::new("chaining");
    index.populate_line_tables(repo.inner(), &mut |done, total| {
        lines_bar.update(done, total)
    })?;
    lines_bar.finish();

    // Dense index pass is opt-in (--embed): a full re-embed can take minutes.
    let sessions_embedded = if embed { run_embed_pass(repo_path)? } else { 0 };

    ui::action(format!(
        "rebuilt derived state: {} commit(s) scanned, {} link(s) written ({} manual replayed), {} line object(s), index: {} session(s){}",
        report.commits_scanned,
        report.links_written,
        manual_replayed,
        report.line_objects,
        sessions_indexed,
        if sessions_embedded > 0 {
            format!(", embedded: {sessions_embedded} session(s)")
        } else {
            String::new()
        },
    ));
    ui::action(format!(
        "wiped {} note(s) and {} line-object ref(s); manual links predating the event log are not recoverable",
        report.notes_deleted, report.line_object_refs_deleted,
    ));

    log.append(
        Utc::now(),
        "rebuild",
        Outcome::Ok,
        serde_json::json!({
            "commits_scanned": report.commits_scanned,
            "links_written": report.links_written,
            "manual_replayed": manual_replayed,
            "line_objects": report.line_objects,
            "notes_deleted": report.notes_deleted,
            "line_object_refs_deleted": report.line_object_refs_deleted,
            "sessions_indexed": sessions_indexed,
            "sessions_embedded": sessions_embedded,
        }),
    );
    Ok(())
}

/// Bring state left by an older version up to date.
///
/// Reports every step it considered, not only the ones that changed something:
/// after a rename the interesting answer is often "nothing here needed moving",
/// and a silent command cannot distinguish that from having done nothing.
pub fn upgrade(repo_path: &Path, dry_run: bool) -> Result<()> {
    // A repository is optional: the config directory is machine-global, so this
    // has to work from a home directory that is not a checkout.
    let context = migrate::Context {
        repo_path: open_repo(repo_path).ok().map(|_| repo_path.to_path_buf()),
    };

    if dry_run {
        let pending = migrate::pending(&context)?;
        if pending.is_empty() {
            ui::action("up to date");
            return Ok(());
        }
        ui::action("would run:");
        for (migration, step) in pending {
            ui::row(migration, step);
        }
        return Ok(());
    }

    let reports = migrate::apply_pending(&context)?;
    if reports.is_empty() {
        ui::action("up to date");
        return Ok(());
    }

    for report in &reports {
        let (verb, detail) = match &report.outcome {
            migrate::Outcome::Applied(detail) => ("applied", detail),
            migrate::Outcome::Skipped(detail) => ("skipped", detail),
        };
        let verb = if verb == "applied" {
            ui::ok(verb)
        } else {
            ui::dim(verb)
        };
        ui::indent(format!("{verb}  {}  {}", report.step, detail));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> SessionSummary {
        SessionSummary {
            id: "01ABC".into(),
            title: "Lineage RLS audit".into(),
            agent: "claude".into(),
            turns: 12,
            started_at: "2026-07-26T09:31:04+00:00".into(),
            model: Some("claude-opus-5".into()),
            models_used: vec![],
            git_branch: None,
            parent_session_id: None,
            is_sidechain: false,
            is_fork: false,
            vendor_session_id: None,
            prompted_by_email: None,
            prompted_by_name: None,
        }
    }

    #[test]
    fn a_row_names_the_day_and_the_author_so_a_long_list_can_be_scanned() {
        let mut s = summary();
        s.prompted_by_name = Some("Alice".into());
        let row = list_row(&s);
        assert!(row.contains("2026-07-26"), "{row}");
        assert!(row.contains("Alice"), "{row}");
        assert!(row.contains("Lineage RLS audit"), "{row}");
        assert!(
            !row.contains("09:31"),
            "the time of day only crowds it: {row}"
        );
    }

    #[test]
    fn a_row_falls_back_to_the_email_then_omits_the_author_entirely() {
        let mut s = summary();
        s.prompted_by_email = Some("alice@example.dev".into());
        assert!(list_row(&s).contains("alice@example.dev"));
        assert!(!list_row(&summary()).contains('@'));
    }

    /// A fork is the one session kind a reader must not mistake for an ordinary
    /// one: continuing it continues someone else's work.
    #[test]
    fn a_fork_says_so_in_the_list() {
        let mut s = summary();
        s.is_fork = true;
        assert!(list_row(&s).contains("(fork)"));
    }

    #[test]
    fn a_short_and_a_long_title_share_a_column_so_the_date_lines_up() {
        let short = summary();
        let mut long = summary();
        long.title = "A much longer session title than the first one can hold".into();
        long.id = "01LONG".into();
        let rows = list_rows(&[short, long]);
        let day_at: Vec<usize> = rows
            .iter()
            .map(|line| {
                let bytes = line.find("2026-07-26").expect(line);
                line[..bytes].chars().count()
            })
            .collect();
        assert_eq!(day_at[0], day_at[1], "{rows:?}");
        assert!(rows[1].contains('…'), "{}", rows[1]);
    }

    #[test]
    fn a_row_shortens_the_id_so_the_title_gets_the_width() {
        let mut s = summary();
        s.id = "6a1a60a912b8eee6df7f9b661c".into();
        let row = list_row(&s);
        assert!(row.contains("6a1a60a9…"), "{row}");
        assert!(!row.contains("6a1a60a912b8eee6df7f9b661c"), "{row}");
    }
}
