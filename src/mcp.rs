//! Lightweight MCP (Model Context Protocol) server over stdio.
//!
//! Maps the live `serve` session control plane to first-class MCP tools so
//! agents (Claude Code, Codex, OpenCode, …) can navigate / annotate / read
//! decisions without shelling out to multi-step CLI.
//!
//! Transport: newline-delimited JSON-RPC 2.0 on stdin/stdout (MCP stdio).
//! Protocol version: `2024-11-05`.
//!
//! No extra crate dependencies — feature-gated as `mcp` so constrained builds
//! can omit the surface entirely (`--no-default-features` / without `mcp`).

#![cfg(feature = "mcp")]

use std::io::{self, BufRead, Write};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Official protocol version string advertised in `initialize`.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// Server identity for MCP hosts.
const SERVER_NAME: &str = "next-hunk";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Run the MCP server until stdin closes. Writes responses to stdout.
pub fn run_mcp_server() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let lines = stdin.lock().lines();

    for line in lines {
        let line = line.context("read MCP stdin line")?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let msg: JsonRpcMessage = match serde_json::from_str(line) {
            Ok(m) => m,
            Err(e) => {
                // Malformed request with no id → notification-style drop, but
                // try to emit a parse error when possible.
                write_response(
                    &mut stdout,
                    &JsonRpcResponse::error(None, -32700, format!("parse error: {e}")),
                )?;
                continue;
            }
        };

        // Notifications have no `id` and must not receive a response.
        if msg.id.is_none() {
            handle_notification(&msg)?;
            continue;
        }

        let response = handle_request(&msg);
        write_response(&mut stdout, &response)?;
    }
    Ok(())
}

fn handle_notification(msg: &JsonRpcMessage) -> Result<()> {
    // `notifications/initialized` and others: no-op for now.
    let _ = msg.method.as_deref();
    Ok(())
}

fn handle_request(msg: &JsonRpcMessage) -> JsonRpcResponse {
    let id = msg.id.clone();
    let method = match msg.method.as_deref() {
        Some(m) => m,
        None => return JsonRpcResponse::error(id, -32600, "missing method".into()),
    };

    match method {
        "initialize" => JsonRpcResponse::ok(id, initialize_result()),
        "ping" => JsonRpcResponse::ok(id, json!({})),
        "tools/list" => JsonRpcResponse::ok(id, json!({ "tools": tool_defs() })),
        "tools/call" => match call_tool(msg.params.as_ref()) {
            Ok(result) => JsonRpcResponse::ok(id, result),
            Err(e) => {
                // Tool-level errors are returned as successful RPC with
                // isError=true content (MCP convention), so hosts can surface
                // the message without treating it as a protocol failure.
                JsonRpcResponse::ok(id, tool_error_result(&format!("{e:#}")))
            }
        },
        // Empty surfaces so hosts that probe resources/prompts don't fail.
        "resources/list" => JsonRpcResponse::ok(id, json!({ "resources": [] })),
        "prompts/list" => JsonRpcResponse::ok(id, json!({ "prompts": [] })),
        other => JsonRpcResponse::error(id, -32601, format!("method not found: {other}")),
    }
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            "tools": { "listChanged": false }
        },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": SERVER_VERSION
        },
        "instructions": "next-hunk MCP maps live `serve` sessions to tools. Start `next-hunk serve` in a worktree, then use list_sessions → review_structure / navigate / add_comment / get_decision / push_focus_note / reload. Optional `hash` selects among multi-worktree sessions."
    })
}

/// Tool catalog (minimal set from WXB-23).
pub fn tool_defs() -> Vec<Value> {
    vec![
        tool(
            "list_sessions",
            "List live next-hunk serve sessions (hash, socket, repo root, file count). Use before other tools when multiple worktrees may be open.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        ),
        tool(
            "review_structure",
            "Return the current review's file/hunk structure as JSON (same shape as `next-hunk review` / `inspect --json`). Requires a live serve.",
            json!({
                "type": "object",
                "properties": {
                    "hash": {
                        "type": "string",
                        "description": "Optional 16-char session hash from list_sessions (defaults to current worktree)."
                    }
                },
                "additionalProperties": false
            }),
        ),
        tool(
            "navigate",
            "Scroll the human's serve TUI to a file, line, or hunk. Target syntax matches CLI --focus: `<path>`, `<path>:<line>`, or `<path>:h<n>`.",
            json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Navigation target: path / path:line / path:hN"
                    },
                    "hash": { "type": "string", "description": "Optional session hash" }
                },
                "required": ["target"],
                "additionalProperties": false
            }),
        ),
        tool(
            "add_comment",
            "Add a comment on a file in the live review (optionally line, line range, or hunk). Same validation as `next-hunk comment add` (unknown paths error).",
            json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "File path as shown in the review" },
                    "text": { "type": "string", "description": "Comment body" },
                    "line": { "type": "integer", "minimum": 1, "description": "New-side line (range start)" },
                    "line_end": { "type": "integer", "minimum": 1, "description": "Inclusive end of line range" },
                    "hunk": { "type": "integer", "minimum": 1, "description": "1-based hunk ordinal" },
                    "hash": { "type": "string" }
                },
                "required": ["file", "text"],
                "additionalProperties": false
            }),
        ),
        tool(
            "get_decision",
            "Read the human's per-hunk accept/reject decisions from the live serve (same JSON shape as `next-hunk decision`).",
            json!({
                "type": "object",
                "properties": {
                    "hash": { "type": "string" }
                },
                "additionalProperties": false
            }),
        ),
        tool(
            "push_focus_note",
            "Push a focus and/or agent notes into the live serve TUI (same as `next-hunk push --focus … --note …`). Note specs: `path:line=text`, `path:hN=text`, `banner=text`.",
            json!({
                "type": "object",
                "properties": {
                    "focus": {
                        "type": "string",
                        "description": "Optional focus target (path / path:line / path:hN)"
                    },
                    "notes": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional note specs (CLI --note grammar)"
                    },
                    "hash": { "type": "string" }
                },
                "additionalProperties": false
            }),
        ),
        tool(
            "reload",
            "Reload the serve session's diff content. Requires serve started with --watch (or a reloader).",
            json!({
                "type": "object",
                "properties": {
                    "hash": { "type": "string" }
                },
                "additionalProperties": false
            }),
        ),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

fn call_tool(params: Option<&Value>) -> Result<Value> {
    let params = params.context("tools/call missing params")?;
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .context("tools/call missing name")?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    #[cfg(all(feature = "serve", unix))]
    {
        call_tool_impl(name, &args)
    }
    #[cfg(not(all(feature = "serve", unix)))]
    {
        let _ = args;
        // Still validate tool names so hosts get a clear "unknown tool" rather
        // than a blanket OS error. Known session tools share the platform matrix
        // message (docs/PLATFORMS.md).
        match name {
            "list_sessions"
            | "review_structure"
            | "navigate"
            | "add_comment"
            | "get_decision"
            | "push_focus_note"
            | "reload" => {
                bail!("{}", crate::platform::live_session_unavailable(name));
            }
            _ => bail!(
                "unknown tool '{name}' (live MCP session tools need serve+Unix; see docs/PLATFORMS.md)"
            ),
        }
    }
}

#[cfg(all(feature = "serve", unix))]
fn call_tool_impl(name: &str, args: &Value) -> Result<Value> {
    use crate::session_client;

    let hash = args.get("hash").and_then(|v| v.as_str());

    match name {
        "list_sessions" => {
            let sessions = session_client::list_sessions()?;
            tool_text_result(&session_client::json_ok(&sessions)?)
        }
        "review_structure" => {
            let summary = session_client::review_structure(hash)?;
            tool_text_result(&serde_json::to_string_pretty(&summary)?)
        }
        "navigate" => {
            let target = args
                .get("target")
                .and_then(|v| v.as_str())
                .context("navigate requires target")?;
            session_client::navigate(target, hash)?;
            tool_text_result(&format!("ok: navigated to {target}"))
        }
        "add_comment" => {
            let file = args
                .get("file")
                .and_then(|v| v.as_str())
                .context("add_comment requires file")?
                .to_string();
            let text = args
                .get("text")
                .and_then(|v| v.as_str())
                .context("add_comment requires text")?
                .to_string();
            let line = args.get("line").and_then(|v| v.as_u64()).map(|n| n as u32);
            let line_end = args
                .get("line_end")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32);
            let hunk = args
                .get("hunk")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            let id = session_client::add_comment(file, text, line, line_end, hunk, hash)?;
            tool_text_result(&format!("ok: comment added with id {id}"))
        }
        "get_decision" => {
            let decisions = session_client::get_decision(hash)?;
            tool_text_result(&serde_json::to_string_pretty(&decisions)?)
        }
        "push_focus_note" => {
            let focus = args.get("focus").and_then(|v| v.as_str());
            let notes: Vec<String> = args
                .get("notes")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            session_client::push_focus_note(focus, &notes, hash)?;
            tool_text_result("ok: pushed to running server")
        }
        "reload" => {
            session_client::reload(hash)?;
            tool_text_result("ok: session reloaded")
        }
        other => bail!("unknown tool: {other}"),
    }
}

/// Success tool payload. Only used by the Unix+serve tool implementations;
/// non-Unix builds return a fixed error from `call_tool` before calling this.
#[cfg(all(feature = "serve", unix))]
fn tool_text_result(text: &str) -> Result<Value> {
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false
    }))
}

fn tool_error_result(message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true
    })
}

fn write_response(stdout: &mut impl Write, response: &JsonRpcResponse) -> Result<()> {
    let mut line = serde_json::to_string(response)?;
    line.push('\n');
    stdout.write_all(line.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

// ── JSON-RPC wire types ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct JsonRpcMessage {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: Option<String>,
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
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

impl JsonRpcResponse {
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Option<Value>, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError { code, message }),
        }
    }
}

// ── Unit tests (protocol only; no live serve) ───────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_catalog_has_required_minimum_set() {
        let names: Vec<String> = tool_defs()
            .iter()
            .filter_map(|t| {
                t.get("name")
                    .and_then(|n| n.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        for required in [
            "list_sessions",
            "review_structure",
            "navigate",
            "add_comment",
            "get_decision",
            "push_focus_note",
            "reload",
        ] {
            assert!(
                names.iter().any(|n| n == required),
                "missing tool {required} in {names:?}"
            );
        }
    }

    #[test]
    fn initialize_advertises_tools_capability() {
        let init = initialize_result();
        assert_eq!(init["protocolVersion"], PROTOCOL_VERSION);
        assert!(init["capabilities"]["tools"].is_object());
        assert_eq!(init["serverInfo"]["name"], "next-hunk");
    }

    #[test]
    fn handle_tools_list_returns_catalog() {
        let msg = JsonRpcMessage {
            jsonrpc: Some("2.0".into()),
            id: Some(json!(1)),
            method: Some("tools/list".into()),
            params: None,
        };
        let resp = handle_request(&msg);
        let result = resp.result.expect("result");
        let tools = result["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), tool_defs().len());
    }

    #[test]
    fn handle_unknown_method_is_jsonrpc_error() {
        let msg = JsonRpcMessage {
            jsonrpc: Some("2.0".into()),
            id: Some(json!(2)),
            method: Some("nope/method".into()),
            params: None,
        };
        let resp = handle_request(&msg);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[test]
    fn unknown_tool_returns_is_error_content() {
        let msg = JsonRpcMessage {
            jsonrpc: Some("2.0".into()),
            id: Some(json!(3)),
            method: Some("tools/call".into()),
            params: Some(json!({
                "name": "does_not_exist",
                "arguments": {}
            })),
        };
        let resp = handle_request(&msg);
        let result = resp.result.expect("tool errors are RPC-ok with isError");
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("unknown tool")
                || text.contains("require")
                || text.contains("unavailable")
                || text.contains("PLATFORMS"),
            "{text}"
        );
    }

    #[test]
    fn navigate_without_target_is_tool_error() {
        let msg = JsonRpcMessage {
            jsonrpc: Some("2.0".into()),
            id: Some(json!(4)),
            method: Some("tools/call".into()),
            params: Some(json!({
                "name": "navigate",
                "arguments": {}
            })),
        };
        let resp = handle_request(&msg);
        let result = resp.result.expect("result");
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        // Unix+serve: missing `target` arg. Non-Unix: live session unavailable
        // (docs/PLATFORMS.md) — still an isError tool result.
        assert!(
            text.contains("target")
                || text.contains("require")
                || text.contains("server")
                || text.contains("unavailable")
                || text.contains("Unix")
                || text.contains("PLATFORMS"),
            "{text}"
        );
    }

    #[test]
    fn response_serializes_without_null_fields() {
        let ok = JsonRpcResponse::ok(Some(json!(1)), json!({"x": 1}));
        let s = serde_json::to_string(&ok).unwrap();
        assert!(!s.contains("\"error\""));
        let err = JsonRpcResponse::error(Some(json!(1)), -1, "e".into());
        let s = serde_json::to_string(&err).unwrap();
        assert!(!s.contains("\"result\""));
    }
}
