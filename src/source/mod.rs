//! Source adapters: produce unified-diff text for the IR layer.
//!
//! - **Git** is accessed via **gix** (gitoxide) — no `git` CLI subprocess.
//! - **Jujutsu** is accessed via the **`jj` CLI**, emitting `--git` unified
//!   diffs that re-enter the same IR parse path (performance gate preserved).
//!
//! Prefer the VCS-agnostic entry points ([`detect_workspace`], [`produce_diff`],
//! [`produce_show`], [`produce_file_diff`]) from the CLI. Low-level `git_*` /
//! `jj_*` helpers remain available for tests and specialized callers.

mod detect;
mod git;
mod jj;

pub use detect::{detect_workspace, find_workspace, VcsKind, Workspace};
pub use git::{
    find_repo, git_diff, git_diff_produced, git_file_diff, git_show, list_repo_worktree_roots,
    open_repo, ProducedDiff,
};
pub use jj::{jj_available, jj_diff_produced, jj_show};

// Re-export so callers can `use next_hunk::source::VcsPreference`.
pub use crate::config::VcsPreference;

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::DiffScope;

/// Produce a worktree / staged / working-set review for the given workspace.
pub fn produce_diff(
    ws: &Workspace,
    scope: DiffScope,
    pathspecs: &[String],
    include_untracked: bool,
) -> Result<ProducedDiff> {
    match ws.kind {
        VcsKind::Git => git_diff_produced(&ws.root, scope, pathspecs, include_untracked),
        VcsKind::Jj => jj_diff_produced(&ws.root, scope, pathspecs, include_untracked),
    }
}

/// Produce a revision or range review (`show`).
pub fn produce_show(ws: &Workspace, rev: &str) -> Result<String> {
    match ws.kind {
        VcsKind::Git => git_show(&ws.root, rev),
        VcsKind::Jj => jj_show(&ws.root, rev),
    }
}

/// Diff two files on disk.
///
/// Git workspaces use the gix engine (same as before). Pure jj (or when git
/// open fails) falls back to the system `diff -u` so no `.git` is required.
pub fn produce_file_diff(ws: &Workspace, old_path: &Path, new_path: &Path) -> Result<String> {
    match ws.kind {
        VcsKind::Git => {
            let repo = open_repo(&ws.root)?;
            git_file_diff(&repo, old_path, new_path)
        }
        VcsKind::Jj => system_file_diff(old_path, new_path, Some(&ws.root)),
    }
}

/// VCS-agnostic two-file unified diff via `diff -u` (no repository needed).
///
/// Relative paths resolve against `workdir` when provided, else the process cwd.
pub fn system_file_diff(
    old_path: &Path,
    new_path: &Path,
    workdir: Option<&Path>,
) -> Result<String> {
    let resolve = |p: &Path| -> PathBuf {
        if p.is_absolute() {
            p.to_owned()
        } else if let Some(wd) = workdir {
            wd.join(p)
        } else {
            p.to_owned()
        }
    };
    let old_abs = resolve(old_path);
    let new_abs = resolve(new_path);

    if !old_abs.is_file() {
        bail!("old file not found: {}", old_abs.display());
    }
    if !new_abs.is_file() {
        bail!("new file not found: {}", new_abs.display());
    }

    let old_label = path_label(&old_abs, workdir);
    let new_label = path_label(&new_abs, workdir);

    let output = Command::new("diff")
        .args([
            "-u",
            "--label",
            &format!("a/{old_label}"),
            "--label",
            &format!("b/{new_label}"),
        ])
        .arg(&old_abs)
        .arg(&new_abs)
        .output()
        .context(
            "failed to run `diff` for filediff (install GNU/BSD diff, or use a git workspace)",
        )?;

    // diff exits 0 = identical, 1 = different, >1 = error
    let code = output.status.code().unwrap_or(2);
    if code > 1 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("diff failed (exit {code}): {}", stderr.trim());
    }

    let mut text = String::from_utf8(output.stdout).context("diff output is not valid UTF-8")?;
    if text.trim().is_empty() {
        return Ok(String::new());
    }

    // Prepend a git-style file header so the IR parser treats this as one file.
    // `diff -u` already emits ---/+++ / @@ hunks; our parser also accepts that,
    // but a `diff --git` header keeps rename/origin plumbing consistent.
    if !text.starts_with("diff --git ") {
        let header = format!("diff --git a/{old_label} b/{new_label}\n");
        text.insert_str(0, &header);
    }
    Ok(text)
}

fn path_label(abs: &Path, workdir: Option<&Path>) -> String {
    let rel = workdir
        .and_then(|wd| abs.strip_prefix(wd).ok())
        .unwrap_or(abs);
    rel.to_string_lossy().replace('\\', "/")
}

/// Discover a workspace root path (auto VCS). Prefer [`detect_workspace`] when
/// the caller needs the backend kind.
pub fn find_repo_root(start: &Path, pref: VcsPreference) -> Result<PathBuf> {
    Ok(detect_workspace(start, pref)?.root)
}
