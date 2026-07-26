use std::path::Path;

use lineage_core::{
    normalize_repo_path_unscoped, Artifact, ArtifactKind, ArtifactResolve, ResolveStrategy,
    ToolCall,
};
use serde_json::Value;

pub fn extract_text_content(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                let t = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match t {
                    "text" | "input_text" | "output_text" => {
                        item.get("text").and_then(|v| v.as_str()).map(String::from)
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

pub fn extract_cursor_content(
    message: &Value,
    workspace_root: Option<&Path>,
) -> (String, Vec<ToolCall>, Vec<Artifact>) {
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut artifacts = Vec::new();

    let content = message.get("content").unwrap_or(message);
    let items = match content {
        Value::Array(arr) => arr.clone(),
        Value::String(s) => {
            text_parts.push(s.clone());
            return (text_parts.join("\n"), tool_calls, artifacts);
        }
        _ => Vec::new(),
    };

    for item in items {
        let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "text" => {
                if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                    text_parts.push(t.to_string());
                }
            }
            "image" | "image_url" => {
                artifacts.extend(artifacts_from_image_block(&item));
            }
            "tool_use" | "tool-call" => {
                let name = item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool")
                    .to_string();
                let id = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&name)
                    .to_string();
                let input = item
                    .get("input")
                    .or_else(|| item.get("arguments"))
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                tool_calls.push(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: input.clone(),
                    result: None,
                });
                artifacts.extend(artifacts_from_tool_input(
                    &name,
                    item.get("input").or_else(|| item.get("arguments")),
                    workspace_root,
                ));
            }
            _ => {}
        }
    }

    (text_parts.join("\n"), tool_calls, artifacts)
}

pub fn extract_claude_content(
    message: &Value,
    workspace_root: Option<&Path>,
) -> (String, Vec<ToolCall>, Vec<Artifact>, bool) {
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut artifacts = Vec::new();
    let mut is_tool_result = false;

    let content = message.get("content").unwrap_or(message);
    let items = match content {
        Value::Array(arr) => arr,
        Value::String(s) => {
            return (s.clone(), tool_calls, artifacts, false);
        }
        _ => return (String::new(), tool_calls, artifacts, false),
    };

    for item in items {
        let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "text" => {
                if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                    text_parts.push(t.to_string());
                }
            }
            "image" | "image_url" => {
                artifacts.extend(artifacts_from_image_block(item));
            }
            "tool_use" => {
                let name = item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool")
                    .to_string();
                let id = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&name)
                    .to_string();
                let input = item.get("input").map(|v| v.to_string()).unwrap_or_default();
                tool_calls.push(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: input.clone(),
                    result: None,
                });
                artifacts.extend(artifacts_from_tool_input(
                    &name,
                    item.get("input"),
                    workspace_root,
                ));
            }
            "tool_result" => {
                is_tool_result = true;
                let id = item
                    .get("tool_use_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool_result")
                    .to_string();
                let result = item
                    .get("content")
                    .map(|c| match c {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                tool_calls.push(ToolCall {
                    id,
                    name: "tool_result".into(),
                    arguments: String::new(),
                    result: Some(result.chars().take(2000).collect()),
                });
            }
            _ => {}
        }
    }

    (text_parts.join("\n"), tool_calls, artifacts, is_tool_result)
}

pub fn artifacts_from_image_block(item: &Value) -> Vec<Artifact> {
    let url = item
        .get("image_url")
        .and_then(|v| {
            v.get("url")
                .and_then(|u| u.as_str())
                .or_else(|| v.as_str())
                .map(String::from)
        })
        .or_else(|| {
            item.get("source").and_then(|src| {
                if src.get("type").and_then(|t| t.as_str()) == Some("base64") {
                    let media = src
                        .get("media_type")
                        .and_then(|m| m.as_str())
                        .unwrap_or("image/png");
                    let data = src.get("data").and_then(|d| d.as_str())?;
                    Some(format!("data:{media};base64,{data}"))
                } else {
                    None
                }
            })
        })
        .or_else(|| item.get("url").and_then(|v| v.as_str()).map(String::from))
        .or_else(|| item.get("data").and_then(|v| v.as_str()).map(String::from));

    let Some(url) = url else {
        return Vec::new();
    };

    let mime = if url.starts_with("data:") {
        url.strip_prefix("data:")
            .and_then(|s| s.split(';').next())
            .map(String::from)
    } else {
        guess_mime_from_path(&url)
    };

    let kind = if url.contains("screenshot") {
        ArtifactKind::Screenshot
    } else if mime.as_deref().is_some_and(|m| m.starts_with("image/")) {
        ArtifactKind::Image
    } else {
        ArtifactKind::Diagram
    };

    vec![media_artifact(kind, url, mime)]
}

pub fn artifacts_from_tool_input(
    name: &str,
    input: Option<&Value>,
    workspace_root: Option<&Path>,
) -> Vec<Artifact> {
    let Some(input) = input else {
        return Vec::new();
    };

    let lower = name.to_lowercase();
    if lower.contains("generateimage") || lower == "generate_image" {
        let path = input
            .get("filename")
            .or_else(|| input.get("path"))
            .or_else(|| input.get("file"))
            .and_then(|v| v.as_str())
            .unwrap_or("generated.png")
            .to_string();
        return vec![media_artifact(
            ArtifactKind::Image,
            normalize_repo_path_unscoped(&path, workspace_root),
            guess_mime_from_path(&path),
        )];
    }

    // Shell tools carry a `command`, not a file path — recover file writes
    // (heredocs) from the command text; otherwise a terminal-command artifact.
    if is_shell_tool(&lower) {
        return shell_artifacts(input, workspace_root);
    }

    let path = input
        .get("path")
        .or_else(|| input.get("file_path"))
        .or_else(|| input.get("file"))
        .or_else(|| input.get("target"))
        .and_then(|v| v.as_str())
        .map(String::from);

    if lower.contains("patch") || lower == "apply_patch" {
        if let Some(patch) = input
            .get("patch")
            .or_else(|| input.get("diff"))
            .and_then(|v| v.as_str())
        {
            let path = path.unwrap_or_else(|| "unknown".into());
            return vec![artifact_with_resolve(
                ArtifactKind::Diff,
                normalize_repo_path_unscoped(&path, workspace_root),
                None,
                ArtifactResolve {
                    strategy: ResolveStrategy::DiffHunk,
                    old_string: None,
                    new_string: None,
                    patch: Some(patch.to_string()),
                },
            )];
        }
    }

    if let Some(path) = path {
        let line_range = input
            .get("line_range")
            .or_else(|| input.get("range"))
            .and_then(parse_line_range)
            .or_else(|| parse_offset_limit(input));

        if is_edit_tool(&lower) {
            if let Some(old_string) = pick_string(input, &["old_string", "old_str", "oldText"]) {
                return vec![artifact_with_resolve(
                    ArtifactKind::Diff,
                    normalize_repo_path_unscoped(&path, workspace_root),
                    line_range,
                    ArtifactResolve {
                        strategy: ResolveStrategy::OldString,
                        old_string: Some(old_string),
                        new_string: pick_string(input, &["new_string", "new_str", "newText"]),
                        patch: None,
                    },
                )];
            }
        }

        if is_write_tool(&lower) {
            return vec![artifact_with_resolve(
                ArtifactKind::FileEdit,
                normalize_repo_path_unscoped(&path, workspace_root),
                line_range,
                ArtifactResolve {
                    strategy: ResolveStrategy::FullFile,
                    old_string: None,
                    new_string: None,
                    patch: None,
                },
            )];
        }

        // Read-style tools produce no artifact: artifacts represent produced
        // output, and counting reads as file_edit polluted authorship signals
        // (files_written, the link gate, oracle evidence) and the
        // materialization funnel (conversation-schema-v0 "Artifact").
        if is_read_tool(&lower) {
            return Vec::new();
        }

        let kind = if lower.contains("patch") || lower.eq_ignore_ascii_case("edit") {
            ArtifactKind::Diff
        } else {
            ArtifactKind::FileEdit
        };

        return vec![Artifact {
            kind,
            path: normalize_repo_path_unscoped(&path, workspace_root),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range,
            resolve: None,
        }];
    }

    Vec::new()
}

fn is_shell_tool(name: &str) -> bool {
    matches!(name, "shell" | "bash" | "exec_command") || name.contains("terminal")
}

/// A shell invocation yields either the file writes recovered from its command
/// (heredocs — the post-image is in the text, so these materialize like any
/// edit) or, failing that, a single terminal-command artifact preserving the
/// command for context.
fn shell_artifacts(input: &Value, workspace_root: Option<&Path>) -> Vec<Artifact> {
    let command = input
        .get("command")
        .or_else(|| input.get("cmd"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let writes = crate::shell_writes::parse_shell_writes(command);
    if !writes.is_empty() {
        return writes
            .into_iter()
            .map(|w| {
                artifact_with_resolve(
                    ArtifactKind::FileEdit,
                    normalize_repo_path_unscoped(&w.path, workspace_root),
                    None,
                    ArtifactResolve {
                        strategy: w.strategy,
                        old_string: w.old_string,
                        new_string: Some(w.new_string),
                        patch: None,
                    },
                )
            })
            .collect();
    }

    vec![Artifact {
        kind: ArtifactKind::TerminalCommand,
        path: command.to_string(),
        blob_ref: None,
        content_hash: None,
        mime_type: None,
        preview_data_url: None,
        line_range: None,
        resolve: None,
    }]
}

fn artifact_with_resolve(
    kind: ArtifactKind,
    path: String,
    line_range: Option<[u32; 2]>,
    resolve: ArtifactResolve,
) -> Artifact {
    Artifact {
        kind,
        path,
        blob_ref: None,
        content_hash: None,
        mime_type: None,
        preview_data_url: None,
        line_range,
        resolve: Some(resolve),
    }
}

fn is_edit_tool(name: &str) -> bool {
    matches!(
        name,
        "strreplace" | "edit" | "search_replace" | "replace" | "multiedit" | "edit_file"
    ) || name.contains("replace")
}

fn is_read_tool(name: &str) -> bool {
    matches!(
        name,
        "read" | "read_file" | "readfile" | "cat" | "view" | "open" | "notebookread"
    ) || name.contains("grep")
        || name.contains("glob")
        || name.contains("search")
        || name.starts_with("read")
        || name.starts_with("view")
        || name.contains("list")
}

fn is_write_tool(name: &str) -> bool {
    matches!(
        name,
        "write" | "write_file" | "create_file" | "writefile" | "create"
    )
}

fn pick_string(v: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn parse_offset_limit(input: &Value) -> Option<[u32; 2]> {
    let offset = input.get("offset")?.as_u64()? as u32;
    let limit = input.get("limit")?.as_u64()? as u32;
    Some([offset + 1, offset + limit])
}

fn parse_line_range(v: &Value) -> Option<[u32; 2]> {
    if let Some(arr) = v.as_array() {
        if arr.len() >= 2 {
            let a = arr[0].as_u64()? as u32;
            let b = arr[1].as_u64()? as u32;
            return Some([a, b]);
        }
    }
    None
}

fn media_artifact(kind: ArtifactKind, path: String, mime_type: Option<String>) -> Artifact {
    Artifact {
        kind,
        path,
        blob_ref: None,
        content_hash: None,
        mime_type,
        preview_data_url: None,
        line_range: None,
        resolve: None,
    }
}

fn guess_mime_from_path(path: &str) -> Option<String> {
    let lower = path.to_lowercase();
    if lower.ends_with(".png") {
        Some("image/png".into())
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        Some("image/jpeg".into())
    } else if lower.ends_with(".gif") {
        Some("image/gif".into())
    } else if lower.ends_with(".webp") {
        Some("image/webp".into())
    } else if lower.ends_with(".svg") {
        Some("image/svg+xml".into())
    } else {
        None
    }
}

/// Extract markdown image links and embedded data URLs from turn text.
pub fn extract_images_from_text(text: &str) -> Vec<Artifact> {
    let mut artifacts = Vec::new();
    for line in text.lines() {
        for (url, alt) in markdown_image_urls(line) {
            let kind = if alt.to_lowercase().contains("screenshot") || url.contains("screenshot") {
                ArtifactKind::Screenshot
            } else {
                ArtifactKind::Image
            };
            let mime = if url.starts_with("data:") {
                url.strip_prefix("data:")
                    .and_then(|s| s.split(';').next())
                    .map(String::from)
            } else {
                guess_mime_from_path(&url)
            };
            artifacts.push(media_artifact(kind, url, mime));
        }
    }
    artifacts
}

pub fn enrich_turn_with_images(content: &str, artifacts: &mut Vec<Artifact>) {
    for image in extract_images_from_text(content) {
        if !artifacts
            .iter()
            .any(|a| a.path == image.path && a.kind == image.kind)
        {
            artifacts.push(image);
        }
    }
}

fn markdown_image_urls(line: &str) -> Vec<(String, String)> {
    let mut urls = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find("![") {
        let after = &rest[start + 2..];
        let Some(close_bracket) = after.find(']') else {
            break;
        };
        let alt = after[..close_bracket].trim().to_string();
        let after_bracket = &after[close_bracket + 1..];
        if !after_bracket.starts_with('(') {
            rest = &after[close_bracket + 1..];
            continue;
        }
        let inner = &after_bracket[1..];
        let Some(close_paren) = inner.find(')') else {
            break;
        };
        let url = inner[..close_paren].trim();
        if !url.is_empty() {
            urls.push((url.to_string(), alt));
        }
        rest = &inner[close_paren + 1..];
    }
    urls
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_image_block_data_url() {
        let item = json!({
            "type": "image",
            "source": { "type": "base64", "media_type": "image/png", "data": "abc" }
        });
        let arts = artifacts_from_image_block(&item);
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].kind, ArtifactKind::Image);
        assert!(arts[0].path.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn parses_generate_image_tool() {
        let input = json!({ "filename": "out/diagram.png", "description": "x" });
        let arts = artifacts_from_tool_input("GenerateImage", Some(&input), None);
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].path, "out/diagram.png");
        assert_eq!(arts[0].kind, ArtifactKind::Image);
    }

    #[test]
    fn parses_markdown_image() {
        let text = "Here is ![screenshot](assets/screen.png) output";
        let arts = extract_images_from_text(text);
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].kind, ArtifactKind::Screenshot);
    }

    #[test]
    fn extract_text_content_flattens_array_blocks() {
        let content = json!([
            { "type": "text", "text": "hello" },
            { "type": "output_text", "text": "world" }
        ]);
        assert_eq!(extract_text_content(&content), "hello\nworld");
    }

    #[test]
    fn extract_cursor_content_parses_tool_use() {
        let message = json!({
            "content": [
                { "type": "text", "text": "done" },
                { "type": "tool_use", "name": "edit", "input": { "path": "src/lib.rs" } }
            ]
        });
        let (text, tools, _) = extract_cursor_content(&message, None);
        assert_eq!(text, "done");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "edit");
    }

    #[test]
    fn strreplace_absolute_path_becomes_repo_relative() {
        let root = std::path::Path::new("/Users/dev/my-project");
        let input = json!({
            "path": "/Users/dev/my-project/src/auth.rs",
            "old_string": "fn old() {}",
            "new_string": "fn new() {}"
        });
        let arts = artifacts_from_tool_input("StrReplace", Some(&input), Some(root));
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].path, "src/auth.rs");
        assert_eq!(arts[0].kind, ArtifactKind::Diff);
    }

    #[test]
    fn edit_tools_capture_the_post_image() {
        let input = json!({
            "file_path": "src/auth.rs",
            "old_string": "fn old() {}",
            "new_string": "fn new() {}"
        });
        let arts = artifacts_from_tool_input("Edit", Some(&input), None);
        assert_eq!(arts.len(), 1);
        let resolve = arts[0].resolve.as_ref().unwrap();
        assert_eq!(resolve.old_string.as_deref(), Some("fn old() {}"));
        assert_eq!(resolve.new_string.as_deref(), Some("fn new() {}"));
    }

    #[test]
    fn read_style_tools_produce_no_artifacts() {
        for tool in ["Read", "Grep", "Glob", "NotebookRead", "codebase_search"] {
            let input = json!({ "file_path": "src/auth.rs" });
            let arts = artifacts_from_tool_input(tool, Some(&input), None);
            assert!(arts.is_empty(), "{tool} must not produce an artifact");
        }
        // Unknown tools with a path keep the coverage-biased fallback.
        let input = json!({ "file_path": "src/auth.rs" });
        assert_eq!(
            artifacts_from_tool_input("MysteryTool", Some(&input), None).len(),
            1
        );
    }

    #[test]
    fn bash_heredoc_becomes_a_materializable_file_edit() {
        let input = json!({
            "command": "cat > src/auth.rs << 'EOF'\nfn login() {}\nEOF"
        });
        let arts = artifacts_from_tool_input("Bash", Some(&input), None);
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].kind, ArtifactKind::FileEdit);
        assert_eq!(arts[0].path, "src/auth.rs");
        let resolve = arts[0].resolve.as_ref().unwrap();
        // new_string is the post-image the gap-11 resolver anchors on.
        assert_eq!(resolve.new_string.as_deref(), Some("fn login() {}"));
    }

    #[test]
    fn bash_without_a_write_stays_a_terminal_command() {
        let input = json!({ "command": "cargo build && git status" });
        let arts = artifacts_from_tool_input("Bash", Some(&input), None);
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].kind, ArtifactKind::TerminalCommand);
    }

    #[test]
    fn bash_commit_heredoc_produces_no_file_edit() {
        // The false-positive trap end-to-end: no FileEdit for a commit message.
        let input = json!({
            "command": "git commit -m \"$(cat <<'EOF'\nfeat: x\nEOF\n)\""
        });
        let arts = artifacts_from_tool_input("Bash", Some(&input), None);
        assert!(arts.iter().all(|a| a.kind != ArtifactKind::FileEdit));
    }
}
