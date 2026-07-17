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

/// Stable 16-char hex hash of a worktree (or bare repo) root used in socket names.
///
/// Uses a canonical absolute path when the directory exists so `/path` and
/// `/path/` (or symlink aliases) share one session; falls back to the input
/// path when canonicalize fails (e.g. path not yet on disk in tests).
pub fn runtime_socket_hash(repo_root: &Path) -> String {
    let key = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Compute the Unix socket path a `serve` process should bind for `repo_root`.
///
/// Path is deterministic **per worktree root** (not the shared `.git` common
/// dir), so parallel agents in linked worktrees each get an independent
/// session. A `push`/`decision` CLI process in the same worktree finds the
/// same socket without an explicit `--socket` flag:
/// `$XDG_RUNTIME_DIR/next-hunk-<hash>.sock` (fallback `/tmp/next-hunk-<hash>.sock`
/// when `XDG_RUNTIME_DIR` is unset).
///
/// `<hash>` is a stable `DefaultHasher` of the **canonical** worktree root
/// (via [`runtime_socket_hash`]) — good enough to disambiguate worktrees
/// without pulling in a hashing crate. Canonicalization matters on macOS
/// where `/var/folders` vs `/private/var/folders` would otherwise break
/// auto-forward socket matching for headless `diff --focus`.
pub fn runtime_socket_path(repo_root: &Path) -> PathBuf {
    let hash = runtime_socket_hash(repo_root);
    let name = format!("next-hunk-{hash}.sock");
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join(name);
        }
    }
    // Fallback: /tmp keyed only by the worktree hash. The hash already
    // disambiguates worktrees; multi-user same-path collisions on a shared
    // host are rare and handled by the stale-socket probe in
    // ServerListener::spawn. Keeping the path deterministic in repo_root alone
    // is what lets a `push`/`decision` process in the same worktree find the
    // socket.
    PathBuf::from(format!("/tmp/next-hunk-{hash}.sock"))
}

/// Discover live next-hunk server sockets by scanning well-known runtime
/// directories. Returns a list of `(socket_path, repo_hash)` pairs for sockets
/// where a connect succeeds (i.e. a live server is running).
#[cfg(all(feature = "serve", unix))]
pub fn discover_live_sockets() -> Vec<(PathBuf, String)> {
    let mut candidates = Vec::new();
    // Scan XDG_RUNTIME_DIR if set.
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        if !xdg.is_empty() {
            if let Ok(entries) = std::fs::read_dir(&xdg) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    if let Some(name) = name.to_str() {
                        if let Some(hash) = name
                            .strip_prefix("next-hunk-")
                            .and_then(|s| s.strip_suffix(".sock"))
                        {
                            candidates.push((entry.path(), hash.to_string()));
                        }
                    }
                }
            }
        }
    }
    // Also scan /tmp for next-hunk-*.sock files.
    if let Ok(entries) = std::fs::read_dir("/tmp") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if let Some(name) = name.to_str() {
                if let Some(hash) = name
                    .strip_prefix("next-hunk-")
                    .and_then(|s| s.strip_suffix(".sock"))
                {
                    candidates.push((entry.path(), hash.to_string()));
                }
            }
        }
    }
    // Deduplicate by hash (prefer XDG path over /tmp).
    let mut seen = std::collections::HashSet::new();
    let mut live = Vec::new();
    for (path, hash) in candidates {
        if !seen.insert(hash.clone()) {
            continue;
        }
        // Probe: is the socket live?
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            live.push((path, hash));
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

    // ---- runtime_socket_path ----

    // `runtime_socket_path` reads the process-global XDG_RUNTIME_DIR, so tests
    // that touch it race under parallel execution. Every test in this group
    // takes this lock to get a consistent view of the environment.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn socket_path_is_deterministic_per_repo() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // Same repo root → same path (so push/decision can find the server).
        let a = runtime_socket_path(Path::new("/repo/one"));
        let b = runtime_socket_path(Path::new("/repo/one"));
        assert_eq!(a, b);
    }

    #[test]
    fn socket_path_differs_across_repos() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let a = runtime_socket_path(Path::new("/repo/one"));
        let b = runtime_socket_path(Path::new("/repo/two"));
        assert_ne!(a, b);
    }

    #[test]
    fn socket_path_differs_across_worktree_paths() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // Linked worktrees are different paths of the same logical repo —
        // each must get its own socket so parallel `serve` does not collide.
        let main = runtime_socket_path(Path::new("/home/you/project"));
        let linked = runtime_socket_path(Path::new("/home/you/project-feature"));
        assert_ne!(main, linked);
        assert_ne!(
            runtime_socket_hash(Path::new("/home/you/project")),
            runtime_socket_hash(Path::new("/home/you/project-feature"))
        );
    }

    #[test]
    fn socket_hash_is_stable_16_hex() {
        let h = runtime_socket_hash(Path::new("/repo/stable"));
        assert_eq!(h.len(), 16);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(h, runtime_socket_hash(Path::new("/repo/stable")));
    }

    #[test]
    fn socket_path_prefers_xdg_runtime_dir() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // When XDG_RUNTIME_DIR is set, the socket lands there (not /tmp).
        std::env::set_var("XDG_RUNTIME_DIR", "/tmp/xdg-test-fixture");
        let path = runtime_socket_path(Path::new("/repo/x"));
        std::env::remove_var("XDG_RUNTIME_DIR");
        assert!(
            path.starts_with("/tmp/xdg-test-fixture"),
            "expected XDG_RUNTIME_DIR prefix, got {}",
            path.display()
        );
    }

    #[test]
    fn socket_path_filename_contains_next_hunk() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("XDG_RUNTIME_DIR");
        let path = runtime_socket_path(Path::new("/repo/y"));
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        assert!(
            name.starts_with("next-hunk-") && name.ends_with(".sock"),
            "expected next-hunk-*.sock filename, got {name}"
        );
    }

    #[test]
    fn socket_path_collapses_symlink_aliases() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // Existing dir: raw path and its canonicalize() form must map to the
        // same socket (macOS /var ↔ /private/var, and any symlink worktree).
        let dir = std::env::temp_dir().join(format!(
            "nh-sock-canon-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = runtime_socket_path(&dir);
        let canon = runtime_socket_path(&dir.canonicalize().expect("canonicalize temp dir"));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(raw, canon, "socket path must be stable under path aliasing");
    }

    #[cfg(all(feature = "serve", unix))]
    #[test]
    fn discover_live_sockets_empty_when_no_sessions() {
        // With no next-hunk server running, discovery returns empty.
        let sessions = discover_live_sockets();
        // May return empty or find unrelated sockets; at minimum it shouldn't panic.
        assert!(
            sessions.iter().all(|(_, h)| h.len() == 16),
            "all hashes should be 16-char hex strings"
        );
    }
}
