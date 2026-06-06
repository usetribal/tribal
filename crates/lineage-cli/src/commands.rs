use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use lineage_adapters::all_adapters;
use lineage_core::{
    conversation_modified_code, generate_architecture_summary, AgentKind, CommitMappingMode,
    LastImportState, LineageId, LineageRepoConfig,
};
use lineage_git::{
    best_commit_for_conversation, blame_with_lineage, delete_session, ensure_gitattributes,
    hydrate_media_artifacts, link_session_to_commit, list_session_ids, lfs_fetch, lfs_push,
    lfs_status, map_commit_to_sessions, materialize_session_at_commit, open_repo, persist_import,
    purge_orphans, read_conversation, read_repo_config, remap_orphaned_commits, run_doctor,
    stamp_prompted_by, write_last_import, write_repo_config, PROMPTED_BY_EMAIL, PROMPTED_BY_NAME,
};
use lineage_policy::{apply_policy, is_private_session, policy_from_repo_config, prepare_for_export, PolicyConfig};
use lineage_search::{LineageIndex, SearchHit};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn doctor(repo_path: &Path) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let report = run_doctor(&repo)?;

    println!("Lineage doctor");
    println!("  git repo:        {}", report.is_git_repo);
    println!("  notes ref:       {}", report.notes_ref_ok);
    println!("  index ref:       {}", report.index_ref_ok);
    println!("  config ref:      {}", report.config_ref_ok);
    println!("  sessions:        {}", report.session_count);
    if !report.missing_lfs_blobs.is_empty() {
        println!("  missing LFS:     {}", report.missing_lfs_blobs.len());
        for b in report.missing_lfs_blobs.iter().take(5) {
            println!("    - {b}");
        }
    }
    if !report.broken_sessions.is_empty() {
        println!("  broken sessions: {}", report.broken_sessions.len());
        for s in &report.broken_sessions {
            println!("    - {s}");
        }
    }
    for w in &report.warnings {
        println!("  warning: {w}");
    }

    if report.ok() {
        println!("status: ok");
    } else {
        println!("status: issues found");
    }
    Ok(())
}

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

    for (kind, adapter) in adapters {
        if !wanted.contains(&kind) {
            continue;
        }
        let sessions = adapter.discover()?;
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
                        c.metadata.insert(
                            "commit_match_score".into(),
                            serde_json::json!(m.score),
                        );
                        c.metadata.insert(
                            "commit_match_signals".into(),
                            serde_json::json!(m.signals),
                        );
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

    let imported_ids: Vec<LineageId> = conversations.iter().map(|c| c.id.clone()).collect();
    write_last_import(inner, &LastImportState::new(imported_ids))?;

    let line_objects: usize = results.iter().map(|r| r.line_objects_written).sum();
    println!(
        "imported {} session(s), {} line object(s), {} skipped, {} error(s)",
        results.len(),
        line_objects,
        skipped,
        errors
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
    let keys = [
        "cursor_session_id",
        "claude_session_id",
        "codex_session_id",
    ];
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
                parent_session_id: conv
                    .parent_session_id
                    .as_ref()
                    .map(|id| id.to_string()),
                is_sidechain: conv
                    .metadata
                    .get("is_sidechain")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                    || conv.parent_session_id.is_some(),
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

    if json {
        println!("{}", serde_json::to_string_pretty(&summaries)?);
    } else {
        for s in summaries {
            let model = s.model.as_deref().unwrap_or("—");
            println!("{}  {}  {} turns  {}", s.id, s.agent, s.turns, model);
        }
    }
    Ok(())
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

pub fn rebuild_index(repo_path: &Path) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let inner = repo.inner();
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
    index.rebuild(inner)?;
    println!("index rebuilt");
    Ok(())
}

pub fn link(repo_path: &Path, session_id: &str, commit_sha: &str) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let id = LineageId::from(session_id);
    let lines = link_session_to_commit(repo.inner(), &id, commit_sha)?;
    println!("linked {session_id} -> {commit_sha} ({lines} line object(s))");
    Ok(())
}

pub fn materialize(
    repo_path: &Path,
    commit: Option<&str>,
    session: Option<&str>,
) -> Result<()> {
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
    for id in session_ids {
        let count = materialize_session_at_commit(inner, &id, &commit_sha)?;
        println!("materialized {count} line object(s) for {id} @ {commit_sha}");
        total += count;
    }
    println!("done ({total} line object(s))");
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
        report.session_id,
        report.notes_updated,
        report.line_objects_deleted,
        report.blobs_purged
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
