//! Shared session client for the live `serve` control plane.
//!
//! Both the CLI (`list` / `review` / `navigate` / …) and the optional MCP
//! surface (`next-hunk mcp`) talk to a running TUI through the same Unix
//! socket protocol (`tui::server::send_command`). Keeping one client path
//! prevents CLI ↔ MCP drift.
//!
//! Gated on `serve` + Unix — the wire protocol is Unix-socket only.

#![cfg(all(feature = "serve", unix))]

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::cli_parse::{
    discover_live_sockets, parse_focus, parse_note, runtime_socket_hash, runtime_socket_path,
};
use crate::config::{Config, VcsPreference};
use crate::ir::ReviewSummary;
use crate::source::detect_workspace;
use crate::tui::app::Selections;
use crate::tui::server::{send_command, ServerCommand, ServerReply};

/// One live serve session (or a discovery row with optional Info enrichment).
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub hash: String,
    pub socket: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_count: Option<usize>,
    /// True when this session's worktree matches the caller's cwd.
    pub current: bool,
}

/// Resolve the current workspace root for socket discovery (honors `vcs` config).
pub fn workspace_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("current_dir")?;
    let cfg = Config::load(&cwd).map_err(|e| anyhow::anyhow!("{e}"))?;
    let pref = cfg
        .vcs
        .as_deref()
        .map(VcsPreference::parse_str)
        .unwrap_or_default();
    Ok(detect_workspace(&cwd, pref)?.root)
}

/// Resolve a socket from an optional session hash, or the cwd worktree.
pub fn resolve_socket(hash: Option<&str>) -> Result<PathBuf> {
    if let Some(h) = hash {
        let sessions = discover_live_sockets();
        match sessions.iter().find(|(_, hh)| hh == h) {
            Some((path, _)) => Ok(path.clone()),
            None => bail!("no live session with hash {h}"),
        }
    } else {
        let repo = workspace_root()?;
        Ok(runtime_socket_path(&repo))
    }
}

/// Discover live sessions and enrich each with `Info` when available.
pub fn list_sessions() -> Result<Vec<SessionInfo>> {
    let sessions = discover_live_sockets();
    let current_hash = workspace_root().ok().map(|root| runtime_socket_hash(&root));

    let mut out = Vec::with_capacity(sessions.len());
    for (path, hash) in sessions {
        let current = current_hash.as_deref() == Some(hash.as_str());
        match send_command(&path, &ServerCommand::Info) {
            Ok(ServerReply::Info {
                repo_path,
                file_count,
            }) => {
                out.push(SessionInfo {
                    hash,
                    socket: path.display().to_string(),
                    repo: Some(repo_path),
                    file_count: Some(file_count),
                    current,
                });
            }
            _ => {
                out.push(SessionInfo {
                    hash,
                    socket: path.display().to_string(),
                    repo: None,
                    file_count: None,
                    current,
                });
            }
        }
    }
    Ok(out)
}

/// Map a connect/protocol failure into a clear "no server" error when applicable.
fn map_connect_err(err: anyhow::Error) -> anyhow::Error {
    let msg = format!("{err:#}");
    if msg.contains("connect to server socket") {
        anyhow::anyhow!(
            "no next-hunk server running in this repo; start one with `next-hunk serve`"
        )
    } else {
        err
    }
}

fn expect_ok(reply: ServerReply, ctx: &str) -> Result<()> {
    match reply {
        ServerReply::Ok => Ok(()),
        ServerReply::Error { message } => bail!("server error: {message}"),
        other => bail!("unexpected server reply for {ctx}: {other:?}"),
    }
}

/// File/hunk structure from a live session (same shape as `next-hunk review`).
pub fn review_structure(hash: Option<&str>) -> Result<ReviewSummary> {
    let socket = resolve_socket(hash)?;
    match send_command(&socket, &ServerCommand::Review).map_err(map_connect_err)? {
        ServerReply::Review(summary) => Ok(summary),
        ServerReply::Error { message } => bail!("server error: {message}"),
        other => bail!("unexpected server reply for review: {other:?}"),
    }
}

/// Navigate the TUI to a focus target (`path` / `path:line` / `path:hN`).
pub fn navigate(target: &str, hash: Option<&str>) -> Result<()> {
    let focus = parse_focus(target)?;
    let socket = resolve_socket(hash)?;
    let reply = send_command(&socket, &ServerCommand::Navigate { target: focus })
        .map_err(map_connect_err)?;
    expect_ok(reply, "navigate")
}

/// Add a session comment; returns the assigned id.
pub fn add_comment(
    file: &str,
    text: &str,
    line: Option<u32>,
    line_end: Option<u32>,
    hunk: Option<usize>,
    hash: Option<&str>,
) -> Result<String> {
    if line_end.is_some() && line.is_none() {
        bail!("line_end requires line");
    }
    let socket = resolve_socket(hash)?;
    match send_command(
        &socket,
        &ServerCommand::CommentAdd {
            file: file.to_string(),
            text: text.to_string(),
            line,
            line_end,
            hunk,
        },
    )
    .map_err(map_connect_err)?
    {
        ServerReply::CommentAdded { id } => Ok(id),
        ServerReply::Error { message } => bail!("server error: {message}"),
        other => bail!("unexpected server reply for comment add: {other:?}"),
    }
}

/// List session comments as JSON-ready values.
pub fn list_comments(hash: Option<&str>) -> Result<serde_json::Value> {
    let socket = resolve_socket(hash)?;
    match send_command(&socket, &ServerCommand::CommentList).map_err(map_connect_err)? {
        ServerReply::CommentList { comments } => Ok(serde_json::to_value(comments)?),
        ServerReply::Error { message } => bail!("server error: {message}"),
        other => bail!("unexpected server reply for comment list: {other:?}"),
    }
}

/// Read the three-bucket decision JSON from a live session.
pub fn get_decision(hash: Option<&str>) -> Result<Selections> {
    let socket = resolve_socket(hash)?;
    match send_command(&socket, &ServerCommand::Decision).map_err(map_connect_err)? {
        ServerReply::Decisions(selections) => Ok(selections),
        ServerReply::Error { message } => bail!("server error: {message}"),
        other => bail!("unexpected server reply for decision: {other:?}"),
    }
}

/// Push focus and/or notes into a live session (same semantics as `next-hunk push`).
pub fn push_focus_note(focus: Option<&str>, notes: &[String], hash: Option<&str>) -> Result<()> {
    let focus_target = focus.map(parse_focus).transpose()?;
    let parsed_notes = notes
        .iter()
        .map(|s| parse_note(s))
        .collect::<Result<Vec<_>>>()?;
    let socket = resolve_socket(hash)?;
    let reply = send_command(
        &socket,
        &ServerCommand::Push {
            focus: focus_target,
            notes: parsed_notes,
        },
    )
    .map_err(map_connect_err)?;
    expect_ok(reply, "push")
}

/// Reload the live session's diff (requires serve started with `--watch`).
pub fn reload(hash: Option<&str>) -> Result<()> {
    let socket = resolve_socket(hash)?;
    let reply = send_command(&socket, &ServerCommand::Reload).map_err(map_connect_err)?;
    expect_ok(reply, "reload")
}

/// True when `path` looks like a next-hunk runtime socket we would open.
#[allow(dead_code)] // used by tests / future MCP cwd overrides
pub fn is_socket_path(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("sock")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{Note, NoteTarget};
    use crate::tui::server::ServerListener;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempSocket {
        path: PathBuf,
    }
    impl TempSocket {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "nh-session-{}-{}-{}.sock",
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

    fn drain_one_ok(listener: ServerListener) {
        let mut reqs = listener.drain();
        while reqs.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(10));
            reqs = listener.drain();
        }
        for r in reqs {
            let _ = r.reply.send(ServerReply::Ok);
        }
    }

    #[test]
    fn push_focus_note_against_live_socket() {
        let sock = TempSocket::new("push");
        let listener = ServerListener::spawn(sock.path.clone()).unwrap();
        let path = sock.path.clone();
        let drainer = std::thread::spawn(move || drain_one_ok(listener));

        // Bypass resolve_socket (cwd-based) and exercise send path via push
        // semantics: open the temp socket with Push.
        let reply = send_command(
            &path,
            &ServerCommand::Push {
                focus: Some(parse_focus("src/a.rs:1").unwrap()),
                notes: vec![Note {
                    target: NoteTarget::Banner,
                    text: "hi".into(),
                }],
            },
        )
        .unwrap();
        assert!(matches!(reply, ServerReply::Ok));
        drainer.join().unwrap();
    }

    #[test]
    fn map_connect_err_rewrites_missing_server() {
        let err = anyhow::anyhow!("connect to server socket /tmp/missing.sock: No such file");
        let mapped = map_connect_err(err);
        assert!(
            mapped.to_string().contains("no next-hunk server running"),
            "got: {mapped}"
        );
    }
}
