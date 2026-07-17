//! VCS discovery: git (`.git`) vs Jujutsu (`.jj`).
//!
//! Detection walks ancestors of `start`. Preference:
//! - [`VcsPreference::Auto`]: if a `.jj` marker is found at the same or a closer
//!   depth than `.git`, choose Jujutsu (colocated workspaces default to jj).
//! - Explicit [`VcsPreference::Git`] / [`VcsPreference::Jj`] require that marker
//!   (or a discoverable git repo for git).

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};

use crate::config::VcsPreference;

/// Resolved backend for a workspace root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsKind {
    Git,
    Jj,
}

impl VcsKind {
    pub fn as_str(self) -> &'static str {
        match self {
            VcsKind::Git => "git",
            VcsKind::Jj => "jj",
        }
    }
}

/// A discovered review workspace (repo/worktree root + backend).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub root: PathBuf,
    pub kind: VcsKind,
}

/// Walk ancestors of `start` and return a workspace according to `pref`.
pub fn detect_workspace(start: &Path, pref: VcsPreference) -> Result<Workspace> {
    let markers = find_vcs_markers(start);
    match pref {
        VcsPreference::Git => {
            if let Some(root) = markers.git_root {
                return Ok(Workspace {
                    root,
                    kind: VcsKind::Git,
                });
            }
            // Fall back to gix discover for linked worktrees / unusual layouts
            // where a bare `.git` file may not walk cleanly from our marker scan.
            // Replace the error (no chain) so user-facing output stays short.
            crate::source::git::find_repo(start)
                .map(|root| Workspace {
                    root,
                    kind: VcsKind::Git,
                })
                .map_err(|_| {
                    anyhow!(
                        "not a git repository (or any parent): {} (vcs=git)",
                        start.display()
                    )
                })
        }
        VcsPreference::Jj => {
            let root = markers.jj_root.ok_or_else(|| {
                anyhow!(
                    "not a Jujutsu workspace (no .jj in parents of {}): set vcs=auto|git or run inside a jj repo",
                    start.display()
                )
            })?;
            Ok(Workspace {
                root,
                kind: VcsKind::Jj,
            })
        }
        VcsPreference::Auto => {
            // Prefer jj when both markers exist (colocated) or only jj is present.
            // Prefer the closer (deeper) marker when they diverge.
            match (markers.jj_root, markers.git_root) {
                (Some(jj), Some(git)) => {
                    // Closer root = longer path components (nested). If equal depth
                    // (colocated), prefer jj so pure-jj semantics work without
                    // requiring the git index layer.
                    let kind = if path_depth(&jj) >= path_depth(&git) {
                        VcsKind::Jj
                    } else {
                        VcsKind::Git
                    };
                    let root = if kind == VcsKind::Jj { jj } else { git };
                    Ok(Workspace { root, kind })
                }
                (Some(jj), None) => Ok(Workspace {
                    root: jj,
                    kind: VcsKind::Jj,
                }),
                (None, Some(git)) => Ok(Workspace {
                    root: git,
                    kind: VcsKind::Git,
                }),
                (None, None) => {
                    // Last chance: gix may still discover a repo (e.g. GIT_DIR).
                    if let Ok(root) = crate::source::git::find_repo(start) {
                        return Ok(Workspace {
                            root,
                            kind: VcsKind::Git,
                        });
                    }
                    bail!(
                        "not a git or jj workspace (or any parent): {}\n\
                         hint: run inside a repository, or set `vcs = \"git\"|\"jj\"` in config",
                        start.display()
                    )
                }
            }
        }
    }
}

/// Like [`detect_workspace`] with [`VcsPreference::Auto`].
pub fn find_workspace(start: &Path) -> Result<Workspace> {
    detect_workspace(start, VcsPreference::Auto)
}

#[derive(Debug, Default)]
struct VcsMarkers {
    git_root: Option<PathBuf>,
    jj_root: Option<PathBuf>,
}

/// Walk from `start` toward the filesystem root collecting the nearest `.git`
/// and `.jj` markers (directory containing the marker).
fn find_vcs_markers(start: &Path) -> VcsMarkers {
    let mut markers = VcsMarkers::default();
    let mut dir = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    // If start is a file path, begin from its parent.
    if dir.is_file() {
        if let Some(parent) = dir.parent() {
            dir = parent.to_path_buf();
        }
    }
    loop {
        if markers.jj_root.is_none() && dir.join(".jj").is_dir() {
            markers.jj_root = Some(dir.clone());
        }
        if markers.git_root.is_none() {
            let git = dir.join(".git");
            // `.git` may be a directory (normal) or a file (linked worktree).
            if git.is_dir() || git.is_file() {
                markers.git_root = Some(dir.clone());
            }
        }
        if markers.jj_root.is_some() && markers.git_root.is_some() {
            break;
        }
        if !dir.pop() {
            break;
        }
    }
    markers
}

fn path_depth(p: &Path) -> usize {
    p.components().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "next-hunk-vcs-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn auto_prefers_jj_when_colocated() {
        let t = TempDir::new();
        fs::create_dir_all(t.0.join(".jj")).unwrap();
        fs::create_dir_all(t.0.join(".git")).unwrap();
        let ws = detect_workspace(&t.0, VcsPreference::Auto).unwrap();
        assert_eq!(ws.kind, VcsKind::Jj);
        assert_eq!(ws.root, t.0.canonicalize().unwrap());
    }

    #[test]
    fn auto_uses_git_only() {
        let t = TempDir::new();
        fs::create_dir_all(t.0.join(".git")).unwrap();
        let ws = detect_workspace(&t.0, VcsPreference::Auto).unwrap();
        assert_eq!(ws.kind, VcsKind::Git);
    }

    #[test]
    fn auto_uses_jj_only() {
        let t = TempDir::new();
        fs::create_dir_all(t.0.join(".jj")).unwrap();
        let ws = detect_workspace(&t.0, VcsPreference::Auto).unwrap();
        assert_eq!(ws.kind, VcsKind::Jj);
    }

    #[test]
    fn force_git_rejects_pure_jj() {
        let t = TempDir::new();
        fs::create_dir_all(t.0.join(".jj")).unwrap();
        let err = detect_workspace(&t.0, VcsPreference::Git).unwrap_err();
        assert!(
            err.to_string().contains("git") || err.to_string().contains("not a git"),
            "err={err}"
        );
    }

    #[test]
    fn force_jj_rejects_pure_git() {
        let t = TempDir::new();
        fs::create_dir_all(t.0.join(".git")).unwrap();
        let err = detect_workspace(&t.0, VcsPreference::Jj).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("jujutsu") || err.to_string().contains(".jj"),
            "err={err}"
        );
    }

    #[test]
    fn nested_jj_inside_git_prefers_closer_jj() {
        let t = TempDir::new();
        fs::create_dir_all(t.0.join(".git")).unwrap();
        let nested = t.0.join("nested");
        fs::create_dir_all(nested.join(".jj")).unwrap();
        let ws = detect_workspace(&nested, VcsPreference::Auto).unwrap();
        assert_eq!(ws.kind, VcsKind::Jj);
        assert_eq!(ws.root, nested.canonicalize().unwrap());
    }
}
