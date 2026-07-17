//! Optional MCP (Model Context Protocol) control plane over stdio.
//!
//! Maps the live `serve` session protocol to first-class tools so hosts that
//! prefer MCP (Claude Code, Codex, OpenCode, …) do not need to shell out to
//! multi-step CLI invocations. Still uses the same Unix socket client as the
//! CLI — no HTTP broker.
//!
//! Enable with `--features mcp` (pulls `serve`). Not in the default feature
//! set so constrained builds stay lean.
//!
//! Wire format: newline-delimited JSON-RPC 2.0 (MCP stdio transport). Never
//! write non-protocol traffic to stdout — log diagnostics on stderr only.

#![cfg(all(feature = "mcp", feature = "serve", unix))]

use std::io::{self, BufRead, Write};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::session;

/// Protocol version we advertise (widely supported by current hosts).
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Run the MCP server until stdin EOF. Returns Ok on clean shutdown.
pub fn run_stdio() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();

    for line in stdin.lock().lines() {
        let line = line.context("read MCP stdin")?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match handle_line(line) {
            HandleResult::Response(v) => {
                writeln!(stdout, "{v}").context("write MCP response")?;
                stdout.flush().context("flush MCP response")?;
            }
            HandleResult::Notification => {}
        }
    }
    Ok(())
}

enum HandleResult {
    Response(Value),
    Notification,
}

fn handle_line(line: &str) -> HandleResult {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            // Malformed JSON without a parseable id → JSON-RPC parse error.
            return HandleResult::Response(rpc_error(
                Value::Null,
                -32700,
                format!("parse error: {e}"),
            ));
        }
    };

    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    // Notifications have no id (or explicit null in some clients).
    let is_notification = id.is_none() || id.as_ref() == Some(&Value::Null);

    if method == "notifications/initialized" || method.starts_with("notifications/") {
        return HandleResult::Notification;
    }

    let id = id.unwrap_or(Value::Null);

    let result = match method {
        "initialize" => Ok(initialize_result()),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list()),
        "tools/call" => call_tool(&params),
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        "" if is_notification => return HandleResult::Notification,
        other => Err(RpcErr {
            code: -32601,
            message: format!("method not found: {other}"),
        }),
    };

    match result {
        Ok(value) => HandleResult::Response(rpc_ok(id, value)),
        Err(e) => HandleResult::Response(rpc_error(id, e.code, e.message)),
    }
}

struct RpcErr {
    code: i64,
    message: String,
}

fn rpc_ok(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn rpc_error(id: Value, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        },
    })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            "tools": {
                "listChanged": false
            }
        },
        "serverInfo": {
            "name": "next-hunk",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": "Control a live next-hunk serve session (Unix socket). Human must run `next-hunk serve` first. Typical loop: list_sessions → review_structure → navigate / push_focus_note → add_comment → get_decision. Errors are structured JSON with an `error` field when tools fail.",
    })
}

fn tools_list() -> Value {
    json!({
        "tools": [
            tool_def(
                "list_sessions",
                "List live next-hunk serve sessions (hash, socket, repo path, file_count, current). Same discovery as `next-hunk list`.",
                json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            ),
            tool_def(
                "review_structure",
                "Get the current review's file/hunk structure as JSON (no full patch text). Same as `next-hunk review`.",
                json!({
                    "type": "object",
                    "properties": {
                        "hash": {
                            "type": "string",
                            "description": "Optional session hash from list_sessions; defaults to the cwd worktree."
                        }
                    },
                    "additionalProperties": false
                }),
            ),
            tool_def(
                "navigate",
                "Scroll the live TUI to a file, line, or hunk. Target syntax: path | path:line | path:hN (1-based hunk).",
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
            tool_def(
                "add_comment",
                "Add a comment on the live session (file, optional line/line_end/hunk). Same as `next-hunk comment add`.",
                json!({
                    "type": "object",
                    "properties": {
                        "file": { "type": "string", "description": "Repo-relative file path in the review" },
                        "text": { "type": "string", "description": "Comment body" },
                        "line": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "New-side source line (range start when line_end is set)"
                        },
                        "line_end": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Inclusive end of a new-side line range (requires line)"
                        },
                        "hunk": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "1-based hunk ordinal"
                        },
                        "hash": { "type": "string", "description": "Optional session hash" }
                    },
                    "required": ["file", "text"],
                    "additionalProperties": false
                }),
            ),
            tool_def(
                "get_decision",
                "Read the human's per-hunk decisions (accepted/rejected/undecided). Same shape as `next-hunk decision`.",
                json!({
                    "type": "object",
                    "properties": {
                        "hash": { "type": "string", "description": "Optional session hash" }
                    },
                    "additionalProperties": false
                }),
            ),
            tool_def(
                "push_focus_note",
                "Push a focus target and/or notes into the live TUI. Focus: path | path:line | path:hN. Notes: path:line=text, path:hN=text, or banner=text (repeatable).",
                json!({
                    "type": "object",
                    "properties": {
                        "focus": {
                            "type": "string",
                            "description": "Optional focus: path / path:line / path:hN"
                        },
                        "notes": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Note specs (same as CLI --note), e.g. banner=summary or src/a.rs:42=why"
                        },
                        "note": {
                            "type": "string",
                            "description": "Single note spec (alternative to notes[])"
                        },
                        "hash": { "type": "string", "description": "Optional session hash" }
                    },
                    "additionalProperties": false
                }),
            ),
            tool_def(
                "reload",
                "Reload the live session's diff content. Requires serve started with --watch. Same as `next-hunk reload`.",
                json!({
                    "type": "object",
                    "properties": {
                        "hash": { "type": "string", "description": "Optional session hash" }
                    },
                    "additionalProperties": false
                }),
            ),
        ]
    })
}

fn tool_def(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}

fn call_tool(params: &Value) -> std::result::Result<Value, RpcErr> {
    let name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| RpcErr {
            code: -32602,
            message: "tools/call: missing name".into(),
        })?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    // MCP allows arguments to be omitted; treat null as {}.
    let args = if args.is_null() { json!({}) } else { args };

    match dispatch_tool(name, &args) {
        Ok(value) => Ok(tool_success(value)),
        Err(e) => Ok(tool_error(&e)),
    }
}

/// Dispatch one tool by name. Returns structured JSON on success.
fn dispatch_tool(name: &str, args: &Value) -> Result<Value> {
    let hash = opt_str(args, "hash");
    match name {
        "list_sessions" => {
            let sessions = session::list_sessions()?;
            Ok(json!({ "sessions": sessions }))
        }
        "review_structure" => {
            let summary = session::review_structure(hash.as_deref())?;
            Ok(serde_json::to_value(summary)?)
        }
        "navigate" => {
            let target = req_str(args, "target")?;
            session::navigate(&target, hash.as_deref())?;
            Ok(json!({ "ok": true, "target": target }))
        }
        "add_comment" => {
            let file = req_str(args, "file")?;
            let text = req_str(args, "text")?;
            let line = opt_u32(args, "line")?;
            let line_end = opt_u32(args, "line_end")?;
            let hunk = opt_usize(args, "hunk")?;
            let id = session::add_comment(&file, &text, line, line_end, hunk, hash.as_deref())?;
            Ok(json!({ "ok": true, "id": id }))
        }
        "get_decision" => {
            let selections = session::get_decision(hash.as_deref())?;
            Ok(serde_json::to_value(selections)?)
        }
        "push_focus_note" => {
            let focus = opt_str(args, "focus");
            let mut notes: Vec<String> = args
                .get("notes")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(one) = opt_str(args, "note") {
                notes.push(one);
            }
            if focus.is_none() && notes.is_empty() {
                bail!("push_focus_note: provide focus and/or notes");
            }
            session::push_focus_note(focus.as_deref(), &notes, hash.as_deref())?;
            Ok(json!({ "ok": true }))
        }
        "reload" => {
            session::reload(hash.as_deref())?;
            Ok(json!({ "ok": true }))
        }
        other => bail!("unknown tool: {other}"),
    }
}

fn tool_success(structured: Value) -> Value {
    let text = serde_json::to_string_pretty(&structured).unwrap_or_else(|_| structured.to_string());
    json!({
        "content": [
            {
                "type": "text",
                "text": text
            }
        ],
        "structuredContent": structured,
        "isError": false
    })
}

fn tool_error(err: &anyhow::Error) -> Value {
    let message = format!("{err:#}");
    let structured = json!({ "error": message });
    json!({
        "content": [
            {
                "type": "text",
                "text": serde_json::to_string_pretty(&structured).unwrap_or(message.clone())
            }
        ],
        "structuredContent": structured,
        "isError": true
    })
}

fn req_str(args: &Value, key: &str) -> Result<String> {
    match args.get(key).and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => Ok(s.to_string()),
        _ => bail!("missing required string argument `{key}`"),
    }
}

fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn opt_u32(args: &Value, key: &str) -> Result<Option<u32>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => {
            let v = n
                .as_u64()
                .context(format!("`{key}` must be a non-negative integer"))?;
            Ok(Some(
                u32::try_from(v).context(format!("`{key}` out of range"))?,
            ))
        }
        Some(_) => bail!("`{key}` must be an integer"),
    }
}

fn opt_usize(args: &Value, key: &str) -> Result<Option<usize>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => {
            let v = n
                .as_u64()
                .context(format!("`{key}` must be a non-negative integer"))?;
            Ok(Some(
                usize::try_from(v).context(format!("`{key}` out of range"))?,
            ))
        }
        Some(_) => bail!("`{key}` must be an integer"),
    }
}

/// Expose tool names for tests / docs generators.
pub fn tool_names() -> Vec<&'static str> {
    vec![
        "list_sessions",
        "review_structure",
        "navigate",
        "add_comment",
        "get_decision",
        "push_focus_note",
        "reload",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FileSummary, HunkSummary, ReviewSummary};
    use crate::tui::app::{CommentEntry, FocusTarget, Selections};
    use crate::tui::server::{ServerListener, ServerReply};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempSocket {
        path: PathBuf,
    }
    impl TempSocket {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "nh-mcp-{}-{}-{}.sock",
                n,
                std::process::id(),
                label
            ));
            let _ = std::fs::remove_file(&path);
            Self { path }
        }
    }
    impl Drop for TempSocket {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn tool_names_match_issue_minimum_set() {
        let names = tool_names();
        for expected in [
            "list_sessions",
            "review_structure",
            "navigate",
            "add_comment",
            "get_decision",
            "push_focus_note",
            "reload",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }
        assert_eq!(names.len(), 7);
    }

    #[test]
    fn tools_list_has_seven_entries() {
        let list = tools_list();
        let tools = list["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 7);
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, tool_names());
    }

    #[test]
    fn initialize_advertises_tools_capability() {
        let init = initialize_result();
        assert_eq!(init["protocolVersion"], PROTOCOL_VERSION);
        assert!(init["capabilities"]["tools"].is_object());
        assert_eq!(init["serverInfo"]["name"], "next-hunk");
    }

    #[test]
    fn handle_initialize_and_tools_list_jsonrpc() {
        let init_line = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        match handle_line(init_line) {
            HandleResult::Response(v) => {
                assert_eq!(v["id"], 1);
                assert!(v["result"]["capabilities"]["tools"].is_object());
            }
            HandleResult::Notification => panic!("expected response, got notification"),
        }

        let list_line = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
        match handle_line(list_line) {
            HandleResult::Response(v) => {
                assert_eq!(v["result"]["tools"].as_array().unwrap().len(), 7);
            }
            HandleResult::Notification => panic!("expected tools/list response"),
        }
    }

    #[test]
    fn unknown_method_returns_jsonrpc_error() {
        let line = r#"{"jsonrpc":"2.0","id":9,"method":"nope","params":{}}"#;
        match handle_line(line) {
            HandleResult::Response(v) => {
                assert_eq!(v["error"]["code"], -32601);
            }
            _ => panic!("expected error response"),
        }
    }

    #[test]
    fn notification_initialized_is_silent() {
        let line = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert!(matches!(handle_line(line), HandleResult::Notification));
    }

    #[test]
    fn tool_call_missing_server_is_tool_error_not_rpc_error() {
        // No serve in this temp env → structured tool error with isError.
        let line = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_decision","arguments":{}}}"#;
        match handle_line(line) {
            HandleResult::Response(v) => {
                assert!(v.get("error").is_none(), "should not be JSON-RPC error");
                let result = &v["result"];
                assert_eq!(result["isError"], true);
                let text = result["content"][0]["text"].as_str().unwrap();
                assert!(
                    text.contains("error")
                        || text.contains("server")
                        || text.contains("no next-hunk"),
                    "unexpected tool error text: {text}"
                );
            }
            _ => panic!("expected response"),
        }
    }

    #[test]
    fn dispatch_unknown_tool_errors() {
        let err = dispatch_tool("not_a_tool", &json!({})).unwrap_err();
        assert!(err.to_string().contains("unknown tool"));
    }

    #[test]
    fn tool_success_wraps_structured_content() {
        // Protocol shape for hosts: text + structuredContent + isError=false.
        // Live socket round-trips are covered by `session` / `tui::server` tests.
        let wrapped = tool_success(json!({"ok": true, "id": "c0"}));
        assert_eq!(wrapped["isError"], false);
        assert!(wrapped["structuredContent"]["ok"].as_bool().unwrap());
        assert_eq!(wrapped["content"][0]["type"], "text");
        assert!(wrapped["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("c0"));
    }

    #[test]
    fn push_and_decision_round_trip_via_send_command() {
        // Keep one end-to-end socket smoke test here so MCP stays wired to the
        // same ServerCommand/ServerReply path as the CLI client.
        let sock = TempSocket::new("tools");
        let listener = ServerListener::spawn(sock.path.clone()).unwrap();
        assert!(
            sock.path.exists(),
            "spawn must create socket at {}",
            sock.path.display()
        );
        let path = sock.path.clone();

        let drainer = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            let mut seen = 0u8;
            while seen < 2 && std::time::Instant::now() < deadline {
                for r in listener.drain() {
                    match r.command {
                        crate::tui::server::ServerCommand::Navigate { .. } => {
                            let _ = r.reply.send(ServerReply::Ok);
                            seen += 1;
                        }
                        crate::tui::server::ServerCommand::Decision => {
                            let _ = r.reply.send(ServerReply::Decisions(Selections {
                                accepted: vec!["a.rs:h1".into()],
                                rejected: vec![],
                                undecided: vec!["b.rs:h1".into()],
                            }));
                            seen += 1;
                        }
                        other => {
                            let _ = r.reply.send(ServerReply::Error {
                                message: format!("unexpected in test: {other:?}"),
                            });
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_eq!(seen, 2, "drainer expected 2 requests");
        });

        // Brief yield so the accept thread is polling.
        std::thread::sleep(std::time::Duration::from_millis(50));

        let reply = crate::tui::server::send_command(
            &path,
            &crate::tui::server::ServerCommand::Navigate {
                target: FocusTarget::FileLine("a.rs".into(), 42),
            },
        )
        .expect("navigate send_command");
        assert!(matches!(reply, ServerReply::Ok));

        let reply =
            crate::tui::server::send_command(&path, &crate::tui::server::ServerCommand::Decision)
                .expect("decision send_command");
        match reply {
            ServerReply::Decisions(s) => {
                assert_eq!(s.accepted, vec!["a.rs:h1".to_string()]);
            }
            other => panic!("unexpected {other:?}"),
        }

        drainer.join().unwrap();
    }

    #[test]
    fn tool_error_sets_is_error_flag() {
        let err = anyhow::anyhow!("server error: no reloader configured");
        let v = tool_error(&err);
        assert_eq!(v["isError"], true);
        assert!(v["structuredContent"]["error"]
            .as_str()
            .unwrap()
            .contains("no reloader"));
    }

    #[test]
    fn review_summary_serializes_for_tool_payload() {
        let summary = ReviewSummary {
            file_count: 1,
            stream_len: 4,
            inserts: 2,
            deletes: 1,
            files: vec![FileSummary {
                display_path: "a.rs".into(),
                old_path: Some("a.rs".into()),
                new_path: Some("a.rs".into()),
                inserts: 2,
                deletes: 1,
                hunks: vec![HunkSummary {
                    header: "@@ -1,1 +1,2 @@".into(),
                    old_start: 1,
                    old_count: 1,
                    new_start: 1,
                    new_count: 2,
                    lines: 3,
                }],
            }],
        };
        let v = serde_json::to_value(&summary).unwrap();
        assert_eq!(v["file_count"], 1);
        assert_eq!(v["files"][0]["display_path"], "a.rs");
    }

    #[test]
    fn comment_entry_shape_stable() {
        let c = CommentEntry {
            id: "c1".into(),
            file: "a.rs".into(),
            text: "range".into(),
            line: Some(1),
            line_end: Some(5),
            hunk: None,
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["line"], 1);
        assert_eq!(v["line_end"], 5);
    }
}
