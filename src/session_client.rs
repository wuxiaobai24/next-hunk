//! Shared client for live `serve` sessions.
//!
//! Used by the CLI (`push` / `decision` / `list` / …) and the optional MCP
//! control plane so protocol behaviour stays in one place (no CLI/MCP drift).

#![cfg(all(feature = "serve", unix))]

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::cli_parse::{
    discover_live_sockets, parse_focus, parse_note, runtime_socket_hash, runtime_socket_path,
};
use crate::config::{Config, VcsPreference};
use crate::source::detect_workspace;
use crate::tui::app::{FocusTarget, Note};
use crate::tui::server::{send_command, ServerCommand, ServerReply};

/// Resolve the current workspace root for socket discovery.
/// Honors project/user `vcs` config so git and jj workspaces share the same path.
pub fn workspace_root_for_socket() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let cfg = Config::load(&cwd).map_err(|e| anyhow::anyhow!("{e}"))?;
    let pref = cfg
        .vcs
        .as_deref()
        .map(VcsPreference::parse_str)
        .unwrap_or_default();
    Ok(detect_workspace(&cwd, pref)?.root)
}

/// Resolve a socket path from an optional session hash or the current repo.
pub fn resolve_socket(hash: Option<&str>) -> Result<PathBuf> {
    if let Some(h) = hash {
        let sessions = discover_live_sockets();
        let found = sessions.iter().find(|(_, hh)| hh == h);
        match found {
            Some((path, _)) => Ok(path.clone()),
            None => bail!("no live session with hash {h}"),
        }
    } else {
        let repo = workspace_root_for_socket()?;
        Ok(runtime_socket_path(&repo))
    }
}

/// One live session as structured data (for MCP / JSON consumers).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionInfo {
    pub hash: String,
    pub socket: String,
    pub repo_path: Option<String>,
    pub file_count: Option<usize>,
    pub current: bool,
}

/// Discover live sessions and enrich each with `Info` when possible.
pub fn list_sessions() -> Result<Vec<SessionInfo>> {
    let sessions = discover_live_sockets();
    let current_hash = workspace_root_for_socket()
        .ok()
        .map(|root| runtime_socket_hash(&root));

    let mut out = Vec::with_capacity(sessions.len());
    for (path, hash) in sessions {
        let current = current_hash.as_deref() == Some(hash.as_str());
        let (repo_path, file_count) = match send_command(&path, &ServerCommand::Info) {
            Ok(ServerReply::Info {
                repo_path,
                file_count,
            }) => (Some(repo_path), Some(file_count)),
            _ => (None, None),
        };
        out.push(SessionInfo {
            hash,
            socket: path.display().to_string(),
            repo_path,
            file_count,
            current,
        });
    }
    Ok(out)
}

/// Turn a socket-connect failure into an actionable "no server" message.
pub fn map_no_server(err: anyhow::Error) -> anyhow::Error {
    let msg = format!("{err:#}");
    if msg.contains("connect to server socket") {
        anyhow::anyhow!(
            "no next-hunk server running in this repo; start one with `next-hunk serve`"
        )
    } else {
        err
    }
}

fn send(socket: &Path, command: ServerCommand) -> Result<ServerReply> {
    send_command(socket, &command).map_err(map_no_server)
}

/// `ServerReply::Error` → Err; otherwise Ok(reply).
fn unwrap_error(reply: ServerReply) -> Result<ServerReply> {
    match reply {
        ServerReply::Error { message } => bail!("server error: {message}"),
        other => Ok(other),
    }
}

pub fn review_structure(hash: Option<&str>) -> Result<serde_json::Value> {
    let socket = resolve_socket(hash)?;
    match unwrap_error(send(&socket, ServerCommand::Review)?)? {
        ServerReply::Review(summary) => Ok(serde_json::to_value(summary)?),
        other => bail!("unexpected server reply: {other:?}"),
    }
}

pub fn navigate(target: &str, hash: Option<&str>) -> Result<()> {
    let focus_target = parse_focus(target)?;
    let socket = resolve_socket(hash)?;
    match unwrap_error(send(
        &socket,
        ServerCommand::Navigate {
            target: focus_target,
        },
    )?)? {
        ServerReply::Ok => Ok(()),
        other => bail!("unexpected server reply: {other:?}"),
    }
}

pub fn add_comment(
    file: String,
    text: String,
    line: Option<u32>,
    line_end: Option<u32>,
    hunk: Option<usize>,
    hash: Option<&str>,
) -> Result<String> {
    let socket = resolve_socket(hash)?;
    match unwrap_error(send(
        &socket,
        ServerCommand::CommentAdd {
            file,
            text,
            line,
            line_end,
            hunk,
        },
    )?)? {
        ServerReply::CommentAdded { id } => Ok(id),
        other => bail!("unexpected server reply: {other:?}"),
    }
}

pub fn get_decision(hash: Option<&str>) -> Result<serde_json::Value> {
    let socket = resolve_socket(hash)?;
    match unwrap_error(send(&socket, ServerCommand::Decision)?)? {
        ServerReply::Decisions(selections) => Ok(serde_json::to_value(selections)?),
        other => bail!("unexpected server reply: {other:?}"),
    }
}

pub fn push_focus_note(focus: Option<&str>, notes: &[String], hash: Option<&str>) -> Result<()> {
    let focus_target: Option<FocusTarget> = focus.map(parse_focus).transpose()?;
    let parsed_notes: Vec<Note> = notes.iter().map(|s| parse_note(s)).collect::<Result<_>>()?;
    if focus_target.is_none() && parsed_notes.is_empty() {
        bail!("push_focus_note requires at least one of focus or notes");
    }
    let socket = resolve_socket(hash)?;
    match unwrap_error(send(
        &socket,
        ServerCommand::Push {
            focus: focus_target,
            notes: parsed_notes,
        },
    )?)? {
        ServerReply::Ok => Ok(()),
        other => bail!("unexpected server reply: {other:?}"),
    }
}

pub fn reload(hash: Option<&str>) -> Result<()> {
    let socket = resolve_socket(hash)?;
    match unwrap_error(send(&socket, ServerCommand::Reload)?)? {
        ServerReply::Ok => Ok(()),
        other => bail!("unexpected server reply: {other:?}"),
    }
}

pub fn session_info(hash: Option<&str>) -> Result<SessionInfo> {
    let socket = resolve_socket(hash)?;
    let current_hash = workspace_root_for_socket()
        .ok()
        .map(|root| runtime_socket_hash(&root));
    // Prefer hash from path when available.
    let hash_str = hash
        .map(|s| s.to_string())
        .or_else(|| {
            socket
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|name| {
                    // next-hunk-<16hex>.sock
                    name.strip_prefix("next-hunk-")
                        .and_then(|rest| rest.strip_suffix(".sock"))
                        .map(|h| h.to_string())
                })
        })
        .unwrap_or_else(|| "unknown".into());

    match unwrap_error(send(&socket, ServerCommand::Info)?)? {
        ServerReply::Info {
            repo_path,
            file_count,
        } => Ok(SessionInfo {
            hash: hash_str.clone(),
            socket: socket.display().to_string(),
            repo_path: Some(repo_path),
            file_count: Some(file_count),
            current: current_hash.as_deref() == Some(hash_str.as_str()),
        }),
        other => bail!("unexpected server reply: {other:?}"),
    }
}

/// Serialize a successful tool payload as pretty JSON text.
pub fn json_ok<T: serde::Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)?)
}

/// Context helper for callers that need the raw path (tests / diagnostics).
pub fn socket_path_for_repo(repo: &Path) -> PathBuf {
    runtime_socket_path(repo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{Note, NoteTarget};
    use crate::tui::server::{ServerListener, ServerReply};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempSocket {
        path: PathBuf,
    }
    impl TempSocket {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "nh-sc-{}-{}-{}.sock",
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

    fn drain_one_ok(listener: ServerListener, reply: ServerReply) {
        std::thread::spawn(move || {
            let mut got = listener.drain();
            while got.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                got = listener.drain();
            }
            for r in got {
                let _ = r.reply.send(reply.clone());
            }
        });
    }

    #[test]
    fn push_via_send_command_round_trip() {
        let sock = TempSocket::new("push");
        let listener = ServerListener::spawn(sock.path.clone()).unwrap();
        drain_one_ok(listener, ServerReply::Ok);

        let reply = send_command(
            &sock.path,
            &ServerCommand::Push {
                focus: None,
                notes: vec![Note {
                    target: NoteTarget::Banner,
                    text: "via session_client".into(),
                }],
            },
        )
        .unwrap();
        assert!(matches!(reply, ServerReply::Ok));
    }

    #[test]
    fn map_no_server_rewrites_connect_errors() {
        let err = anyhow::anyhow!("connect to server socket /tmp/x.sock: No such file");
        let mapped = map_no_server(err);
        let msg = format!("{mapped:#}");
        assert!(msg.contains("no next-hunk server running"), "{msg}");
    }

    #[test]
    fn map_no_server_passes_other_errors() {
        let err = anyhow::anyhow!("parse reply: EOF");
        let mapped = map_no_server(err);
        assert!(format!("{mapped:#}").contains("parse reply"));
    }
}
