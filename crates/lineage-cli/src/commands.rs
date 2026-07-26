use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use lineage_adapters::all_adapters;
use lineage_core::{
    conversation_modified_code, generate_architecture_summary, AgentKind, CommitMappingMode,
    LastImportState, LineageId, LineageRepoConfig,
};
use lineage_git::{
    assemble_batch, best_commit_for_conversation, blame_with_lineage, delete_session,
    ensure_gitattributes, hydrate_media_artifacts, lfs_fetch, lfs_push, lfs_status,
    link_session_to_commit, list_session_ids, map_commit_to_sessions,
    materialize_session_at_commit, open_repo, persist_import, purge_orphans, read_conversation,
    read_conversation_stored, read_repo_config, remap_orphaned_commits, stamp_prompted_by,
    sync_push, write_last_import, write_repo_config, PROMPTED_BY_EMAIL, PROMPTED_BY_NAME,
};
use lineage_policy::{
    apply_policy, is_private_session, policy_from_repo_config, prepare_for_export, PolicyConfig,
};
use lineage_search::{LineageIndex, SearchHit};

use crate::auth;
use crate::events::{EventLog, Outcome};

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
        println!("wrote default config to refs/lineage/config");
        println!("ensured .gitattributes for .lineage/media/** LFS pointers");
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

    let existing_ids: std::collections::HashSet<LineageId> = if incremental {
        list_session_ids(inner)?.into_iter().collect()
    } else {
        std::collections::HashSet::new()
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
        println!("discovered {} {} session(s)", sessions.len(), kind.as_str());
        for session in sessions {
            if let Some(since_dt) = since_dt {
                if session.started_at.map(|t| t < since_dt).unwrap_or(false) {
                    skipped += 1;
                    continue;
                }
            }

            if incremental {
                let id_hint = session.id_hint.clone();
                if existing_ids.iter().any(|id| id.as_str().contains(&id_hint)) {
                    if let Ok(meta) = fs::metadata(&session.source_path) {
                        if let Ok(modified) = meta.modified() {
                            let modified: DateTime<Utc> = modified.into();
                            if let Some(started) = session.started_at {
                                if modified <= started {
                                    skipped += 1;
                                    continue;
                                }
                            }
                        }
                    }
                }
            }

            match adapter.read(&session) {
                Ok(mut conv) => {
                    let source = session.source_path.display().to_string();
                    conv.metadata
                        .insert("source".into(), serde_json::Value::String(source.clone()));
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
    let index = LineageIndex::open(lineage_repo.git_dir().join("lineage").join("index.db"))?;
    for r in &results {
        if let Some(conv) = read_conversation(inner, &r.session_id)? {
            index.index_conversation(&conv)?;
        }
    }
    // Mirror the newly imported sessions' line objects and chain them. Import
    // with --link-head may have just materialized objects; walking them now
    // keeps `context chain` answerable without a full rebuild.
    let imported_ids: Vec<LineageId> = conversations.iter().map(|c| c.id.clone()).collect();
    index.populate_line_tables_for_sessions(inner, &imported_ids)?;
    write_last_import(inner, &LastImportState::new(imported_ids))?;

    let line_objects: usize = results.iter().map(|r| r.line_objects_written).sum();
    println!(
        "imported {} session(s), {} line object(s), {} skipped, {} error(s)",
        results.len(),
        line_objects,
        skipped,
        errors
    );
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
        println!(
            "  {} (blob {}, {} line objects)",
            r.session_id, r.blob_oid, r.line_objects_written
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
struct SessionSummary {
    id: String,
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
            println!("{}", serde_json::to_string_pretty(&summary)?);
        } else {
            for id in ids {
                println!("{id}");
            }
        }
        return Ok(());
    }

    let ids = list_session_ids(inner)?;
    let mut summaries = Vec::new();
    for id in ids {
        if let Some(conv) = read_conversation(inner, &id)? {
            summaries.push(SessionSummary {
                id: conv.id.to_string(),
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
                vendor_session_id: vendor_session_id(&conv),
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
            });
        }
    }

    // Newest first. `list_session_ids` returns ref order, which is neither
    // chronological nor stable across machines, so a user scanning for "the
    // session I ran this morning" had to read every row.
    summaries.sort_by(|a, b| b.started_at.cmp(&a.started_at));

    if json {
        println!("{}", serde_json::to_string_pretty(&summaries)?);
    } else {
        for s in &summaries {
            println!("{}", list_row(s));
        }
    }
    Ok(())
}

/// One row a person can choose from. Id, agent, and turn count identify a
/// session but say nothing about whether it is the one you want; the date and
/// the author are what make a list of 100 sessions navigable. Every field is
/// already on the summary — this only stops discarding it at print time.
fn list_row(s: &SessionSummary) -> String {
    let model = s.model.as_deref().unwrap_or("—");
    let mut row = format!(
        "{}  {}  {:>4} turns  {}  {}",
        s.id,
        list_day(&s.started_at),
        s.turns,
        s.agent,
        model
    );
    if let Some(who) = s
        .prompted_by_name
        .as_deref()
        .or(s.prompted_by_email.as_deref())
    {
        row.push_str(&format!("  {who}"));
    }
    if s.is_fork {
        row.push_str("  (fork)");
    }
    row
}

/// `started_at` is stored RFC 3339. Show the date only: the time of day almost
/// never distinguishes two sessions, and the full stamp crowds out the author.
fn list_day(started_at: &str) -> &str {
    started_at.split('T').next().unwrap_or(started_at)
}

pub fn show(repo_path: &Path, session_id: &str, json: bool, hydrate_images: bool) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let id = LineageId::from(session_id);
    let mut conv = read_conversation(repo.inner(), &id)?
        .ok_or_else(|| format!("session not found: {session_id}"))?;

    if hydrate_images {
        let _ = hydrate_media_artifacts(repo.inner(), &mut conv)?;
    }

    if json {
        println!("{}", conv.to_json()?);
    } else {
        println!("Session: {}", conv.id);
        println!("Agent:   {}", conv.agent.as_str());
        println!("Started: {}", conv.started_at);
        println!("Turns:   {}", conv.turns.len());
        if conv.private {
            println!("Private: true");
        }
        if let Some(origin) = &conv.fork_origin {
            println!(
                "Forked:  from session {} on {}",
                origin.source_session_id,
                origin.forked_at.format("%Y-%m-%d")
            );
        }
        if let Some(model) = conv.primary_model() {
            println!("Model:   {model}");
        }
        if let Some(summary) = conv
            .metadata
            .get("architecture_summary")
            .and_then(|v| v.as_str())
        {
            println!("\nSummary:\n{summary}");
        }
        let models = conv.models_used();
        if models.len() > 1 {
            println!("Models:  {}", models.join(", "));
        }
        let meta_keys = [
            PROMPTED_BY_EMAIL,
            PROMPTED_BY_NAME,
            "claude_code_version",
            "git_branch",
            "codex_cli_version",
            "codex_originator",
            "cursor_session_id",
            "claude_session_id",
            "codex_session_id",
        ];
        for key in meta_keys {
            if let Some(value) = conv.metadata.get(key).and_then(|v| v.as_str()) {
                println!("{key}: {value}");
            }
        }
        for (i, turn) in conv.turns.iter().enumerate() {
            let preview: String = turn.content.chars().take(120).collect();
            let model = turn
                .model
                .as_deref()
                .map(|m| format!(" ({m})"))
                .unwrap_or_default();
            println!("\n[{i}] {:?}{model}: {preview}", turn.role);
            if !turn.tool_calls.is_empty() {
                println!("    tools: {}", turn.tool_calls.len());
            }
        }
    }
    Ok(())
}

pub fn blame(repo_path: &Path, target: &str, json: bool) -> Result<()> {
    let (path, line) = parse_blame_target(target)?;
    let repo = open_repo(repo_path)?;
    let rel = std::path::Path::new(&path);

    let result = blame_with_lineage(repo.inner(), rel, line)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    println!("{}:{} ", path, result.line);
    println!("commit: {}", result.commit_sha);
    println!("sessions: {}", result.sessions.len());
    for id in &result.sessions {
        println!("  - {id}");
    }
    for obj in &result.line_objects {
        println!(
            "  line {}:{} turn {} ({:?})",
            obj.line_range[0], obj.line_range[1], obj.turn_id, obj.confidence
        );
    }
    if !result.matches.is_empty() {
        println!("matches:");
        for m in &result.matches {
            let range = m
                .line_range
                .map(|r| format!("{}:{}", r[0], r[1]))
                .unwrap_or_else(|| "?".into());
            println!(
                "  turn {} lines {range} ({:?}): {}",
                m.turn_id, m.confidence, m.content_preview
            );
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
        "json" => println!("{}", serde_json::to_string_pretty(&out)?),
        "jsonl" => {
            for conv in out {
                println!("{}", serde_json::to_string(&conv)?);
            }
        }
        other => return Err(format!("unsupported format: {other}").into()),
    }
    Ok(())
}

/// Runs the device login against a Lineage server: prints the verification URL
/// and code, polls until the browser approval completes the login server-side,
/// and stores the returned session handle (the durable credential — the JWT it
/// mints is short-lived and re-minted per command).
pub fn login(server: &str) -> Result<()> {
    let start = auth::device_start(server)?;
    println!("To sign in, open this URL in a browser:");
    println!();
    println!("  {}", start.verification_uri_complete);
    println!();
    println!(
        "and confirm code {} (or enter it at {}).",
        start.user_code, start.verification_uri
    );
    println!("Waiting for approval…");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(start.expires_in);
    let mut interval = start.interval;
    loop {
        match auth::device_poll(server, &start.device_code)? {
            auth::DevicePollResponse::Pending { slow_down } => {
                if slow_down {
                    // OAuth device-flow rule: back off by 5s when told to slow down.
                    interval += 5;
                }
            }
            auth::DevicePollResponse::Complete { session_handle, .. } => {
                auth::store_login(server, session_handle)?;
                println!(
                    "Logged in. Credentials stored in {}.",
                    auth::credentials_path()?.display()
                );
                return Ok(());
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err("sign-in request expired before it was approved: run login again".into());
        }
        std::thread::sleep(std::time::Duration::from_secs(interval));
    }
}

// Explicit flag / LINEAGE_TOKEN bypass the stored login entirely (CI, scripts);
// otherwise the stored session handle is exchanged for a fresh 15-minute JWT,
// which comfortably outlives a sync run — no mid-run refresh needed.
fn resolve_sync_token(server: &str, token: Option<&str>) -> Result<String> {
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

pub fn sync(
    repo_path: &Path,
    server: Option<&str>,
    token: Option<&str>,
    remote: &str,
) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let inner = repo.inner();
    let server = auth::resolve_server(server)?;
    let token = resolve_sync_token(&server, token)?;

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
    println!(
        "syncing {} conversation(s), {} line object(s), {} commit link(s), {} blob(s) to {server}",
        batch.conversations.len(),
        batch.line_objects.len(),
        batch.session_commit_links.len(),
        batch.blobs.len()
    );

    let outcome = sync_push(inner, &server, &token, &batch)?;
    let report = &outcome.report;
    println!(
        "synced to repo {} ({} accepted, {} noop, {} rejected, {} pending, {} blob(s) uploaded)",
        report.repo_id,
        report.accepted,
        report.noop,
        report.rejected,
        report.pending,
        report.blobs_uploaded
    );
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
            println!("no results for '{query}'");
            return Ok(());
        }
        print_hits(&hits);
        return Ok(());
    }
    print_hits(&hits);
    Ok(())
}

fn print_hits(hits: &[SearchHit]) {
    for hit in hits {
        println!(
            "{}  score={:.2}  {}",
            hit.session_id, hit.score, hit.snippet
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
        println!("index rebuilt ({embedded} session(s) embedded)");
    } else {
        println!("index rebuilt");
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

/// `git lineage rebuild embeddings`: the dense-embedding backfill on its own,
/// symmetric with `rebuild index`.
pub fn rebuild_embeddings(repo_path: &Path) -> Result<()> {
    let embedded = run_embed_pass(repo_path)?;
    if embedded > 0 {
        println!("embeddings rebuilt ({embedded} session(s) embedded)");
    } else {
        println!("embeddings up to date (0 session(s) embedded)");
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
    println!("linked {session_id} -> {commit_sha} ({lines} line object(s))");
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
        println!("materialized {count} line object(s) for {id} @ {commit_sha}");
        sessions.push(serde_json::json!({ "session_id": id.as_str(), "line_objects": count }));
        total += count;
    }
    println!("done ({total} line object(s))");
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
    println!(
        "remapped {} orphan commit(s) ({} patch-id match(es)), {} session(s), {} line object(s)",
        report.remapped_commits,
        report.patch_id_matches,
        report.rematerialized_sessions,
        report.line_objects_updated
    );
    Ok(())
}

pub fn lfs_status_cmd(repo_path: &Path) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let report = lfs_status(repo.inner())?;
    println!("LFS status");
    println!("  referenced:      {}", report.referenced);
    println!("  present local:   {}", report.present_local);
    println!("  transport refs:  {}", report.transport_refs);
    println!("  git-lfs CLI:     {}", report.git_lfs_available);
    if !report.missing_local.is_empty() {
        println!("  missing local:   {}", report.missing_local.len());
        for b in report.missing_local.iter().take(10) {
            println!("    - {b}");
        }
    }
    Ok(())
}

pub fn lfs_push_cmd(repo_path: &Path, remote: Option<&str>) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let remote = remote.unwrap_or("origin");
    let report = lfs_push(repo.inner(), remote)?;
    println!(
        "pushed LFS objects to {remote} via {} ({} uploaded, {} skipped)",
        report.method, report.uploaded, report.skipped
    );
    Ok(())
}

pub fn delete_session_cmd(repo_path: &Path, session_id: &str, purge_blobs: bool) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let id = LineageId::from(session_id);
    let report = delete_session(repo.inner(), &id, purge_blobs)?;
    println!(
        "deleted session {} ({} note(s) updated, {} line object(s) removed, {} blob(s) purged)",
        report.session_id, report.notes_updated, report.line_objects_deleted, report.blobs_purged
    );
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
    let _ = index.rebuild(repo.inner());
    Ok(())
}

pub fn gc_cmd(repo_path: &Path) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let report = purge_orphans(repo.inner())?;
    println!(
        "purged {} orphan line object(s), {} unreferenced blob(s), {} transport ref(s)",
        report.line_objects_purged, report.blobs_purged, report.transport_refs_purged
    );
    Ok(())
}

pub fn lfs_fetch_cmd(repo_path: &Path, remote: Option<&str>) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let remote = remote.unwrap_or("origin");
    let report = lfs_fetch(repo.inner(), remote)?;
    println!(
        "fetched LFS objects from {remote} via {} ({} downloaded, {} skipped)",
        report.method, report.downloaded, report.skipped
    );
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

    println!(
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
    );
    println!(
        "wiped {} note(s) and {} line-object ref(s); manual links predating the event log are not recoverable",
        report.notes_deleted, report.line_object_refs_deleted,
    );

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

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> SessionSummary {
        SessionSummary {
            id: "01ABC".into(),
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
}
