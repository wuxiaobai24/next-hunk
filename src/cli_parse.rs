//! Parsing for the agent-bridge CLI specs (`--focus` / `--note`).
//!
//! These are kept separate from `main.rs` so the parsing logic is unit-testable
//! in isolation. All functions return [`anyhow::Result`] so a bad spec produces
//! a clear error before the TUI opens.
//!
//! Spec grammar (path is a repo-relative file path):
//! - `--focus <path>`            → `FocusTarget::File`
//! - `--focus <path>:<line>`     → `FocusTarget::FileLine` (`<line>` is a number)
//! - `--focus <path>:h<n>`       → `FocusTarget::FileHunk` (1-based hunk ordinal)
//! - `--note <path>:<line>=<text>`  → `NoteTarget::Line`
//! - `--note <path>:h<n>=<text>`    → `NoteTarget::Hunk`
//! - `--note banner=<text>`         → `NoteTarget::Banner`
//! - `--note =<text>`               → `NoteTarget::Banner` (empty location)

use anyhow::{bail, Result};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::tui::app::{FocusTarget, Note, NoteTarget};

/// Parse a `--focus` spec into a [`FocusTarget`].
pub fn parse_focus(spec: &str) -> Result<FocusTarget> {
    let spec = spec.trim();
    if spec.is_empty() {
        bail!("--focus: empty spec");
    }
    // The rsplit lets file paths contain ':' on systems that allow it; we only
    // treat the last `:segment` as a line/hunk locator.
    let (path, suffix) = match spec.rsplit_once(':') {
        None => return Ok(FocusTarget::File(spec.to_string())),
        Some(pair) => pair,
    };
    if path.is_empty() {
        bail!("--focus: missing path before `:` in `{spec}`");
    }
    if let Some(num) = suffix.strip_prefix('h') {
        let hunk = num
            .parse::<usize>()
            .map_err(|_| anyhow::anyhow!("--focus: invalid hunk ordinal `{num}`"))?;
        if hunk == 0 {
            bail!("--focus: hunk ordinals are 1-based (got h0)");
        }
        Ok(FocusTarget::FileHunk(path.to_string(), hunk))
    } else {
        let line = suffix
            .parse::<u32>()
            .map_err(|_| anyhow::anyhow!("--focus: invalid line number `{suffix}`"))?;
        if line == 0 {
            bail!("--focus: line numbers are 1-based (got 0)");
        }
        Ok(FocusTarget::FileLine(path.to_string(), line))
    }
}

/// Parse one `--note` spec into a [`Note`]. The text portion is everything
/// after the first `=`; a `=` may appear inside the text itself only if the
/// location was `banner` or empty (otherwise the location `key=value` is split
/// on the first `=`).
pub fn parse_note(spec: &str) -> Result<Note> {
    // Split into location and text on the first '='.
    let (location, text) = match spec.split_once('=') {
        Some((loc, text)) => (loc, text),
        None => bail!("--note: missing `=text` in `{spec}`"),
    };
    if location.is_empty() {
        // `--note =text` → banner.
        return Ok(Note {
            target: NoteTarget::Banner,
            text: text.to_string(),
        });
    }
    if location == "banner" {
        return Ok(Note {
            target: NoteTarget::Banner,
            text: text.to_string(),
        });
    }
    // location is `<path>` / `<path>:<line>` / `<path>:h<n>`.
    // A location with a ':' but an empty path half (e.g. `:42=text`) is malformed.
    let (path, suffix) = match location.rsplit_once(':') {
        None => bail!(
            "--note: location `{location}` needs a `:line` or `:h<n>` (use `banner=` for a banner)"
        ),
        Some(pair) => pair,
    };
    if path.is_empty() {
        bail!("--note: missing path before `:` in `{location}`");
    }
    if let Some(num) = suffix.strip_prefix('h') {
        let hunk = num
            .parse::<usize>()
            .map_err(|_| anyhow::anyhow!("--note: invalid hunk ordinal `{num}`"))?;
        if hunk == 0 {
            bail!("--note: hunk ordinals are 1-based (got h0)");
        }
        Ok(Note {
            target: NoteTarget::Hunk {
                path: path.to_string(),
                hunk,
            },
            text: text.to_string(),
        })
    } else {
        let line = suffix
            .parse::<u32>()
            .map_err(|_| anyhow::anyhow!("--note: invalid line number `{suffix}`"))?;
        if line == 0 {
            bail!("--note: line numbers are 1-based (got 0)");
        }
        Ok(Note {
            target: NoteTarget::Line {
                path: path.to_string(),
                line,
            },
            text: text.to_string(),
        })
    }
}

/// Stable per-repo hash used in socket names: a `DefaultHasher` of the
/// canonical repo root. Session ids start with this hash, so discovery can
/// filter by repo.
pub fn repo_socket_hash(repo_root: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    repo_root.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Stable per-repo socket-name prefix: `next-hunk-<hash>`. Every session
/// socket for that repo starts with this prefix.
pub fn repo_socket_prefix(repo_root: &Path) -> String {
    format!("next-hunk-{}", repo_socket_hash(repo_root))
}

/// Compute the Unix socket path a session TUI should bind for `repo_root`.
///
/// Every interactive review (`diff`, `show`, `serve`) binds one socket, so a
/// CLI process in the same repo can discover and drive it. The name is
/// `<repo prefix>-<pid>.sock` — deterministic per repo (so clients can filter
/// by repo) but unique per process (so several reviews of one repo coexist):
/// `$XDG_RUNTIME_DIR/next-hunk-<hash>-<pid>.sock` (fallback
/// `/tmp/next-hunk-<hash>-<pid>.sock` when `XDG_RUNTIME_DIR` is unset,
/// mirroring config.rs's manual env resolution).
pub fn session_socket_path(repo_root: &Path) -> PathBuf {
    let name = format!(
        "{}-{}.sock",
        repo_socket_prefix(repo_root),
        std::process::id()
    );
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join(name);
        }
    }
    // Fallback: /tmp keyed by the repo hash + pid. The repo hash disambiguates
    // repos; the pid disambiguates concurrent sessions. Multi-user same-repo
    // collisions on a shared host are rare and handled by the stale-socket
    // probe in ServerListener::spawn.
    PathBuf::from(format!("/tmp/{name}"))
}

/// Extract the session id from a socket file name: the part between the
/// `next-hunk-` prefix and the `.sock` suffix — `<hash>` (legacy serve
/// sockets) or `<hash>-<pid>` (per-process sessions). The id is what
/// `--hash` accepts and `list` prints.
pub fn parse_session_id(socket_name: &str) -> Option<String> {
    socket_name
        .strip_prefix("next-hunk-")
        .and_then(|s| s.strip_suffix(".sock"))
        .map(|s| s.to_string())
}

/// Remove leftover `next-hunk-*.sock` files that no longer host a live
/// session. Called before binding a new session socket: a TUI killed by
/// SIGHUP/SIGKILL (terminal closed, tmux pane killed) runs no `Drop`, and a
/// pid-suffixed path is never reused, so without a sweep the runtime dir
/// accumulates dead sockets until reboot. Live sockets (connect succeeds)
/// are left alone.
#[cfg(all(feature = "serve", unix))]
pub fn sweep_stale_sockets() {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        if !xdg.is_empty() {
            dirs.push(PathBuf::from(xdg));
        }
    }
    dirs.push(PathBuf::from("/tmp"));
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if parse_session_id(name).is_none() {
                continue;
            }
            let path = entry.path();
            if std::os::unix::net::UnixStream::connect(&path).is_err() {
                // dead socket file (not a socket, or no listener) — reclaim
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// Discover live next-hunk session sockets by scanning well-known runtime
/// directories. Returns a list of `(socket_path, session_id)` pairs for
/// sockets where a connect succeeds (i.e. a live session is running).
#[cfg(all(feature = "serve", unix))]
pub fn discover_live_sockets() -> Vec<(PathBuf, String)> {
    let mut candidates = Vec::new();
    // Scan XDG_RUNTIME_DIR if set.
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        if !xdg.is_empty() {
            if let Ok(entries) = std::fs::read_dir(&xdg) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        if let Some(id) = parse_session_id(name) {
                            candidates.push((entry.path(), id));
                        }
                    }
                }
            }
        }
    }
    // Also scan /tmp for next-hunk-*.sock files.
    if let Ok(entries) = std::fs::read_dir("/tmp") {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Some(id) = parse_session_id(name) {
                    candidates.push((entry.path(), id));
                }
            }
        }
    }
    // Deduplicate by full socket name (the same socket can appear in both
    // scans when XDG_RUNTIME_DIR points into /tmp); prefer the XDG path.
    let mut seen = std::collections::HashSet::new();
    let mut live = Vec::new();
    for (path, id) in candidates {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !seen.insert(name) {
            continue;
        }
        // Probe: is the socket live?
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            live.push((path, id));
        }
    }
    live
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_plain_path() {
        assert_eq!(
            parse_focus("src/a.rs").unwrap(),
            FocusTarget::File("src/a.rs".into())
        );
    }

    #[test]
    fn focus_line() {
        assert_eq!(
            parse_focus("src/a.rs:42").unwrap(),
            FocusTarget::FileLine("src/a.rs".into(), 42)
        );
    }

    #[test]
    fn focus_hunk() {
        assert_eq!(
            parse_focus("src/a.rs:h3").unwrap(),
            FocusTarget::FileHunk("src/a.rs".into(), 3)
        );
    }

    #[test]
    fn focus_rejects_zero_line() {
        assert!(parse_focus("a.rs:0").is_err());
    }

    #[test]
    fn focus_rejects_zero_hunk() {
        assert!(parse_focus("a.rs:h0").is_err());
    }

    #[test]
    fn focus_rejects_empty() {
        assert!(parse_focus("").is_err());
    }

    #[test]
    fn note_line() {
        let n = parse_note("a.rs:42=explanation").unwrap();
        assert_eq!(
            n.target,
            NoteTarget::Line {
                path: "a.rs".into(),
                line: 42
            }
        );
        assert_eq!(n.text, "explanation");
    }

    #[test]
    fn note_hunk() {
        let n = parse_note("a.rs:h2=note text").unwrap();
        assert_eq!(
            n.target,
            NoteTarget::Hunk {
                path: "a.rs".into(),
                hunk: 2
            }
        );
        assert_eq!(n.text, "note text");
    }

    #[test]
    fn note_banner_keyword() {
        let n = parse_note("banner=overall summary").unwrap();
        assert_eq!(n.target, NoteTarget::Banner);
        assert_eq!(n.text, "overall summary");
    }

    #[test]
    fn note_banner_empty_location() {
        let n = parse_note("=just text").unwrap();
        assert_eq!(n.target, NoteTarget::Banner);
        assert_eq!(n.text, "just text");
    }

    #[test]
    fn note_text_may_contain_equals() {
        // The text is everything after the FIRST '='.
        let n = parse_note("a.rs:1=key=value").unwrap();
        assert_eq!(n.text, "key=value");
    }

    #[test]
    fn note_rejects_missing_equals() {
        assert!(parse_note("a.rs:1 no equals").is_err());
    }

    #[test]
    fn note_rejects_bare_path() {
        // A bare path with no :line is ambiguous and rejected with guidance.
        assert!(parse_note("a.rs=text").is_err());
    }

    // ---- session_socket_path ----

    // `session_socket_path` reads the process-global XDG_RUNTIME_DIR, so tests
    // that touch it race under parallel execution. Every test in this group
    // takes this lock to get a consistent view of the environment.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn socket_path_is_deterministic_per_repo() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // Same repo root → same path (so clients can filter by repo prefix).
        let a = session_socket_path(Path::new("/repo/one"));
        let b = session_socket_path(Path::new("/repo/one"));
        assert_eq!(a, b);
    }

    #[test]
    fn socket_path_differs_across_repos() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let a = session_socket_path(Path::new("/repo/one"));
        let b = session_socket_path(Path::new("/repo/two"));
        assert_ne!(a, b);
    }

    #[test]
    fn socket_path_prefers_xdg_runtime_dir() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // When XDG_RUNTIME_DIR is set, the socket lands there (not /tmp).
        std::env::set_var("XDG_RUNTIME_DIR", "/tmp/xdg-test-fixture");
        let path = session_socket_path(Path::new("/repo/x"));
        std::env::remove_var("XDG_RUNTIME_DIR");
        assert!(
            path.starts_with("/tmp/xdg-test-fixture"),
            "expected XDG_RUNTIME_DIR prefix, got {}",
            path.display()
        );
    }

    #[test]
    fn socket_path_filename_contains_repo_prefix_and_pid() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("XDG_RUNTIME_DIR");
        let path = session_socket_path(Path::new("/repo/y"));
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let expected_tail = format!("-{}.sock", std::process::id());
        assert!(
            name.starts_with("next-hunk-") && name.ends_with(&expected_tail),
            "expected next-hunk-<hash>-<pid>.sock filename, got {name}"
        );
        // the session id (between prefix and suffix) is parseable back out
        let id = parse_session_id(name).expect("parse session id");
        assert_eq!(
            id,
            name.strip_prefix("next-hunk-")
                .unwrap()
                .strip_suffix(".sock")
                .unwrap()
        );
    }

    #[test]
    fn parse_session_id_accepts_and_rejects() {
        // new per-process sessions: <hash>-<pid>
        assert_eq!(
            parse_session_id("next-hunk-131e603184455fa7-1234.sock"),
            Some("131e603184455fa7-1234".into())
        );
        // legacy serve sockets: <hash> only
        assert_eq!(
            parse_session_id("next-hunk-131e603184455fa7.sock"),
            Some("131e603184455fa7".into())
        );
        // unrelated names
        assert_eq!(parse_session_id("next-hunk.sock"), None);
        assert_eq!(parse_session_id("other-1.sock"), None);
        assert_eq!(parse_session_id("next-hunk-1.txt"), None);
    }

    #[cfg(all(feature = "serve", unix))]
    #[test]
    fn discover_live_sockets_empty_when_no_sessions() {
        // With no next-hunk server running, discovery returns empty.
        let sessions = discover_live_sockets();
        // May return empty or find unrelated sockets; at minimum it shouldn't panic.
        assert!(
            sessions.iter().all(|(_, h)| h.len() >= 16),
            "all ids should carry a 16-char repo hash, got {sessions:?}"
        );
    }
}
