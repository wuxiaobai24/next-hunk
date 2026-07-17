//! Persist per-hunk review decisions across TUI sessions.
//!
//! Default store: `<git-dir>/next-hunk/decisions-<scope>.json` so state is
//! local to the repository (and worktree when using linked worktrees' private
//! git dirs). Falls back to `$XDG_STATE_HOME/next-hunk/<repo-hash>/…` when the
//! git dir is not available.
//!
//! Keys match `--select` / `decision` JSON: `"{display_path}:h{n}"` (1-based)
//! with values `"accepted"` | `"rejected"`. Undecided hunks are omitted.
//! On load/reload, values are remapped by path:hN (best-effort).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::tui::app::{Decision, ReviewReport};

/// On-disk document version. Bump when the schema changes incompatibly.
const VERSION: u32 = 1;

/// Wire values for a decision (matches agent JSON vocabulary).
fn decision_to_wire(d: Decision) -> Option<&'static str> {
    match d {
        Decision::Accept => Some("accepted"),
        Decision::Reject => Some("rejected"),
        Decision::Undecided => None,
    }
}

fn decision_from_wire(s: &str) -> Option<Decision> {
    match s {
        "accepted" | "accept" | "a" => Some(Decision::Accept),
        "rejected" | "reject" | "r" => Some(Decision::Reject),
        "undecided" | "u" => Some(Decision::Undecided),
        _ => None,
    }
}

/// Serializable review state for one scope (worktree / staged / working-set / …).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedState {
    #[serde(default = "default_version")]
    pub version: u32,
    /// Diff scope label used as part of the filename (informational in body).
    #[serde(default)]
    pub scope: String,
    /// Hunk key → `"accepted"` | `"rejected"`.
    #[serde(default)]
    pub decisions: HashMap<String, String>,
}

fn default_version() -> u32 {
    VERSION
}

impl PersistedState {
    pub fn new(scope: &str) -> Self {
        Self {
            version: VERSION,
            scope: scope.to_string(),
            decisions: HashMap::new(),
        }
    }

    /// Convert wire map into `(key, Decision)` pairs, dropping unknown values.
    pub fn as_decisions(&self) -> HashMap<String, Decision> {
        self.decisions
            .iter()
            .filter_map(|(k, v)| decision_from_wire(v).map(|d| (k.clone(), d)))
            .filter(|(_, d)| !matches!(d, Decision::Undecided))
            .collect()
    }

    /// Replace decisions from a path-keyed map of [`Decision`]s.
    pub fn set_from_decisions(&mut self, decisions: &HashMap<String, Decision>) {
        self.decisions.clear();
        for (k, d) in decisions {
            if let Some(wire) = decision_to_wire(*d) {
                self.decisions.insert(k.clone(), wire.to_string());
            }
        }
    }
}

/// Resolve the store path for `scope` under a git directory.
///
/// Example: `.git/next-hunk/decisions-worktree.json`
pub fn path_in_git_dir(git_dir: &Path, scope: &str) -> PathBuf {
    git_dir
        .join("next-hunk")
        .join(format!("decisions-{}.json", sanitize_scope(scope)))
}

/// Path for the most recent full review export (decisions + comments + notes).
///
/// Example: `.git/next-hunk/last-export.json`
pub fn path_last_export_in_git_dir(git_dir: &Path) -> PathBuf {
    git_dir.join("next-hunk").join("last-export.json")
}

/// XDG fallback for last-export when no git dir is known.
pub fn path_last_export_in_xdg(repo_key: &str) -> Option<PathBuf> {
    let base = xdg_state_home()?;
    Some(
        base.join("next-hunk")
            .join(sanitize_scope(repo_key))
            .join("last-export.json"),
    )
}

/// Resolve where to cache the last quit-time [`ReviewReport`].
///
/// Preference matches decision persistence:
/// 1. `<git_dir>/next-hunk/last-export.json` when `git_dir` is set
/// 2. else discover git from `workdir` via `gix`
/// 3. else XDG state keyed by a hash of the workdir path
pub fn resolve_last_export_path(git_dir: Option<&Path>, workdir: Option<&Path>) -> Option<PathBuf> {
    if let Some(gd) = git_dir {
        return Some(path_last_export_in_git_dir(gd));
    }
    if let Some(wd) = workdir {
        if let Ok(repo) = gix::discover(wd) {
            return Some(path_last_export_in_git_dir(repo.git_dir()));
        }
        let key = repo_key_from_path(wd);
        return path_last_export_in_xdg(&key);
    }
    None
}

/// Write the full review report so agents can recover it via `next-hunk last-export`
/// when they miss stdout (common with `serve` quit on the human's terminal).
///
/// When `workdir` is `None` (e.g. patch path), falls back to the process cwd so
/// agents inside a git/jj worktree still get a recoverable cache.
pub fn save_last_export(workdir: Option<&Path>, report: &ReviewReport) -> std::io::Result<()> {
    let cwd = std::env::current_dir().ok();
    let wd = workdir.or(cwd.as_deref());
    let Some(path) = resolve_last_export_path(None, wd) else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string(report)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&tmp, body)?;
    let _ = fs::remove_file(&path);
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Load the cached last export. Missing file → `None`.
///
/// When `workdir` is `None`, falls back to the process cwd.
pub fn load_last_export(workdir: Option<&Path>) -> Option<ReviewReport> {
    let cwd = std::env::current_dir().ok();
    let wd = workdir.or(cwd.as_deref());
    let path = resolve_last_export_path(None, wd)?;
    let text = fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<ReviewReport>(&text) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("warning: cannot parse last export {}: {e}", path.display());
            None
        }
    }
}

/// XDG fallback when no git dir is known.
pub fn path_in_xdg(repo_key: &str, scope: &str) -> Option<PathBuf> {
    let base = xdg_state_home()?;
    Some(
        base.join("next-hunk")
            .join(sanitize_scope(repo_key))
            .join(format!("decisions-{}.json", sanitize_scope(scope))),
    )
}

fn xdg_state_home() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".local/state"))
}

/// Keep filenames portable: alnum, dash, underscore only.
fn sanitize_scope(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "default".into()
    } else {
        out
    }
}

/// Resolve where to store state given optional git dir and worktree path.
///
/// Preference:
/// 1. `<git_dir>/next-hunk/decisions-<scope>.json` when `git_dir` is set
/// 2. else discover git from `workdir` via `gix` and use that git dir
/// 3. else XDG state keyed by a hash of the workdir path
pub fn resolve_store_path(
    git_dir: Option<&Path>,
    workdir: Option<&Path>,
    scope: &str,
) -> Option<PathBuf> {
    if let Some(gd) = git_dir {
        return Some(path_in_git_dir(gd, scope));
    }
    if let Some(wd) = workdir {
        if let Ok(repo) = gix::discover(wd) {
            return Some(path_in_git_dir(repo.git_dir(), scope));
        }
        // Bare hash of absolute path for non-git contexts.
        let key = repo_key_from_path(wd);
        return path_in_xdg(&key, scope);
    }
    None
}

fn repo_key_from_path(p: &Path) -> String {
    // Stable, short, filesystem-safe key (not cryptographic).
    let s = p
        .canonicalize()
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Load state from disk. Missing file → empty state. Corrupt file → empty + warn.
pub fn load(path: &Path) -> PersistedState {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<PersistedState>(&text) {
            Ok(mut s) => {
                if s.version == 0 {
                    s.version = VERSION;
                }
                s
            }
            Err(e) => {
                eprintln!("warning: cannot parse review state {}: {e}", path.display());
                PersistedState::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => PersistedState::default(),
        Err(e) => {
            eprintln!("warning: cannot read review state {}: {e}", path.display());
            PersistedState::default()
        }
    }
}

/// Atomic-ish write: write to `.tmp` then rename.
pub fn save(path: &Path, state: &PersistedState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&tmp, body)?;
    // On Windows rename over existing can fail; remove first best-effort.
    let _ = fs::remove_file(path);
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_decisions() {
        let mut state = PersistedState::new("worktree");
        let mut map = HashMap::new();
        map.insert("src/a.rs:h1".into(), Decision::Accept);
        map.insert("src/a.rs:h2".into(), Decision::Reject);
        map.insert("src/b.rs:h1".into(), Decision::Undecided);
        state.set_from_decisions(&map);
        assert_eq!(state.decisions.get("src/a.rs:h1").unwrap(), "accepted");
        assert_eq!(state.decisions.get("src/a.rs:h2").unwrap(), "rejected");
        assert!(!state.decisions.contains_key("src/b.rs:h1"));

        let back = state.as_decisions();
        assert_eq!(back.get("src/a.rs:h1"), Some(&Decision::Accept));
        assert_eq!(back.get("src/a.rs:h2"), Some(&Decision::Reject));
        assert!(!back.contains_key("src/b.rs:h1"));
    }

    #[test]
    fn save_and_load_file() {
        let dir = std::env::temp_dir().join(format!(
            "next-hunk-persist-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("decisions-worktree.json");
        let mut state = PersistedState::new("worktree");
        state
            .decisions
            .insert("foo.rs:h1".into(), "accepted".into());
        save(&path, &state).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.scope, "worktree");
        assert_eq!(loaded.decisions.get("foo.rs:h1").unwrap(), "accepted");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_is_empty() {
        let path = PathBuf::from("/tmp/next-hunk-does-not-exist-xyz.json");
        let s = load(&path);
        assert!(s.decisions.is_empty());
    }

    #[test]
    fn sanitize_scope_strips_bad_chars() {
        assert_eq!(sanitize_scope("working-set"), "working-set");
        assert_eq!(sanitize_scope("a/b"), "a_b");
        assert_eq!(sanitize_scope(""), "default");
    }

    #[test]
    fn path_in_git_dir_shape() {
        let p = path_in_git_dir(Path::new("/repo/.git"), "worktree");
        assert_eq!(
            p,
            PathBuf::from("/repo/.git/next-hunk/decisions-worktree.json")
        );
    }

    #[test]
    fn wire_aliases() {
        assert_eq!(decision_from_wire("accepted"), Some(Decision::Accept));
        assert_eq!(decision_from_wire("accept"), Some(Decision::Accept));
        assert_eq!(decision_from_wire("rejected"), Some(Decision::Reject));
        assert_eq!(decision_from_wire("nope"), None);
    }

    #[test]
    fn last_export_path_shape() {
        let p = path_last_export_in_git_dir(Path::new("/repo/.git"));
        assert_eq!(p, PathBuf::from("/repo/.git/next-hunk/last-export.json"));
    }

    #[test]
    fn last_export_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "next-hunk-last-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        // Simulate a bare workdir with no .git → XDG path; force via git_dir API.
        let path = path_last_export_in_git_dir(&dir);
        let report = ReviewReport {
            schema_version: crate::tui::app::REVIEW_REPORT_SCHEMA_VERSION,
            accepted: vec!["a.rs:h1".into()],
            rejected: vec!["b.rs:h1".into()],
            undecided: vec![],
            comments: vec![],
            notes: vec![],
            banner: Some("done".into()),
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let body = serde_json::to_string(&report).unwrap();
        fs::write(&path, &body).unwrap();
        let loaded: ReviewReport =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.accepted, report.accepted);
        assert_eq!(loaded.rejected, report.rejected);
        assert_eq!(loaded.banner, report.banner);
        assert_eq!(loaded.schema_version, report.schema_version);
        let _ = fs::remove_dir_all(&dir);
    }
}
