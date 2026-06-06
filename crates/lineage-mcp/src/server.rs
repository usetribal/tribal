use std::io::{self, BufRead, Write};
use std::path::Path;

use lineage_core::LineageId;
use lineage_git::{
    blame_with_lineage, list_session_ids, materialize_session_at_commit, open_repo,
    read_conversation, read_repo_config, remap_orphaned_commits, run_doctor,
};
use lineage_policy::{apply_policy, policy_from_repo_config, prepare_for_export};
use lineage_search::LineageIndex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

pub async fn run_stdio(repo_path: &Path) -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let req: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                write_response(
                    &mut stdout,
                    Value::Null,
                    None,
                    Some(JsonRpcError {
                        code: -32700,
                        message: e.to_string(),
                    }),
                )?;
                continue;
            }
        };

        let id = req.id.clone().unwrap_or(Value::Null);
        let result = handle_request(repo_path, &req.method, &req.params).await;

        match result {
            Ok(value) => write_response(&mut stdout, id, Some(value), None)?,
            Err(e) => {
                write_response(
                    &mut stdout,
                    id,
                    None,
                    Some(JsonRpcError {
                        code: -32000,
                        message: e,
                    }),
                )?;
            }
        }
    }

    Ok(())
}

fn write_response(
    stdout: &mut io::Stdout,
    id: Value,
    result: Option<Value>,
    error: Option<JsonRpcError>,
) -> io::Result<()> {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result,
        error,
    };
    writeln!(stdout, "{}", serde_json::to_string(&resp).unwrap())?;
    stdout.flush()
}

pub async fn handle_request(repo_path: &Path, method: &str, params: &Value) -> Result<Value, String> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "lineage-mcp", "version": "0.1.0" }
        })),
        "notifications/initialized" => Ok(json!({})),
        "tools/list" => Ok(json!({
            "tools": [
                tool_schema("lineage_list_sessions", "List ingested sessions", json!({}), &[]),
                tool_schema("lineage_get_session", "Get session by ID", json!({
                    "session_id": { "type": "string" },
                    "redact": { "type": "boolean" }
                }), &["session_id"]),
                tool_schema("lineage_blame_line", "Blame a file line", json!({
                    "path": { "type": "string" },
                    "line": { "type": "integer" }
                }), &["path", "line"]),
                tool_schema("lineage_search", "Search sessions", json!({
                    "query": { "type": "string" },
                    "limit": { "type": "integer" }
                }), &["query"]),
                tool_schema("lineage_doctor", "Check lineage repo health", json!({}), &[]),
                tool_schema("lineage_materialize", "Materialize line objects", json!({
                    "session_id": { "type": "string" },
                    "commit_sha": { "type": "string" }
                }), &[]),
                tool_schema("lineage_rebuild_index", "Rebuild search index", json!({}), &[]),
                tool_schema("lineage_export", "Export sessions", json!({
                    "redact": { "type": "boolean" },
                    "format": { "type": "string" }
                }), &[]),
                tool_schema("lineage_remap", "Remap lineage after rebase", json!({}), &[])
            ]
        })),
        "tools/call" => handle_tool_call(repo_path, params).await,
        _ => Err(format!("unknown method: {method}")),
    }
}

fn tool_schema(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required
        }
    })
}

async fn handle_tool_call(repo_path: &Path, params: &Value) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| "missing tool name".to_string())?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    let repo = open_repo(repo_path).map_err(|e| e.to_string())?;
    let inner = repo.inner();
    let repo_config = read_repo_config(inner).map_err(|e| e.to_string())?;
    let policy = policy_from_repo_config(&repo_config);

    let content = match name {
        "lineage_list_sessions" => {
            let ids = list_session_ids(inner).map_err(|e| e.to_string())?;
            let mut sessions = Vec::new();
            for id in ids {
                if let Some(conv) = read_conversation(inner, &id).map_err(|e| e.to_string())? {
                    if conv.private {
                        continue;
                    }
                    sessions.push(json!({
                        "id": conv.id.as_str(),
                        "agent": conv.agent.as_str(),
                        "turns": conv.turns.len(),
                        "started_at": conv.started_at.to_rfc3339(),
                        "model": conv.primary_model(),
                    }));
                }
            }
            serde_json::to_string_pretty(&sessions).unwrap()
        }
        "lineage_get_session" => {
            let session_id = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "session_id required".to_string())?;
            let redact = args.get("redact").and_then(|v| v.as_bool()).unwrap_or(true);
            let id = LineageId::from(session_id);
            let conv = read_conversation(inner, &id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("session not found: {session_id}"))?;
            let out = if redact {
                apply_policy(&policy, conv).conversation
            } else {
                conv
            };
            out.to_json().map_err(|e| e.to_string())?
        }
        "lineage_blame_line" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "path required".to_string())?;
            let line = args
                .get("line")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "line required".to_string())? as u32;
            let full = repo.workdir().join(path);
            let result = blame_with_lineage(inner, &full, line).map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
        }
        "lineage_search" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "query required".to_string())?;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))
                .map_err(|e| e.to_string())?;
            let hits = index.search(query, limit).map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&hits).unwrap()
        }
        "lineage_doctor" => {
            let report = run_doctor(&repo).map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&json!({
                "is_git_repo": report.is_git_repo,
                "notes_ref_ok": report.notes_ref_ok,
                "index_ref_ok": report.index_ref_ok,
                "session_count": report.session_count,
                "broken_sessions": report.broken_sessions,
                "warnings": report.warnings,
            }))
            .unwrap()
        }
        "lineage_materialize" => {
            let session_id = args.get("session_id").and_then(|v| v.as_str());
            let commit_sha = args.get("commit_sha").and_then(|v| v.as_str());
            let commit = if let Some(sha) = commit_sha {
                sha.to_string()
            } else {
                inner
                    .head()
                    .map_err(|e| e.to_string())?
                    .peel_to_commit()
                    .map_err(|e| e.to_string())?
                    .id()
                    .to_string()
            };
            let ids: Vec<LineageId> = if let Some(sid) = session_id {
                vec![LineageId::from(sid)]
            } else {
                list_session_ids(inner).map_err(|e| e.to_string())?
            };
            let mut total = 0usize;
            for id in ids {
                total += materialize_session_at_commit(inner, &id, &commit)
                    .map_err(|e| e.to_string())?;
            }
            serde_json::to_string_pretty(&json!({ "line_objects": total, "commit": commit }))
                .unwrap()
        }
        "lineage_rebuild_index" => {
            let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))
                .map_err(|e| e.to_string())?;
            index.rebuild(inner).map_err(|e| e.to_string())?;
            "\"index rebuilt\"".to_string()
        }
        "lineage_export" => {
            let redact = args.get("redact").and_then(|v| v.as_bool()).unwrap_or(true);
            let mut export_policy = policy.clone();
            if redact {
                export_policy.strip_private = true;
            }
            let ids = list_session_ids(inner).map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            for id in ids {
                if let Some(conv) = read_conversation(inner, &id).map_err(|e| e.to_string())? {
                    if export_policy.strip_private && conv.private {
                        continue;
                    }
                    out.push(prepare_for_export(&export_policy, conv));
                }
            }
            serde_json::to_string_pretty(&out).unwrap()
        }
        "lineage_remap" => {
            let report = remap_orphaned_commits(inner).map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&json!({
                "remapped_commits": report.remapped_commits,
                "rematerialized_sessions": report.rematerialized_sessions,
                "line_objects_updated": report.line_objects_updated,
            }))
            .unwrap()
        }
        other => return Err(format!("unknown tool: {other}")),
    };

    Ok(json!({
        "content": [{ "type": "text", "text": content }]
    }))
}
