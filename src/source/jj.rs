//! Jujutsu (`jj`) source adapter.
//!
//! Invokes the **`jj` CLI** and feeds **git-format unified diffs** into the
//! existing IR parse path. There is no in-process jj library dependency: pure
//! jj workspaces work without a `.git` directory or gix.
//!
//! ## Scope mapping (vs git)
//!
//! | next-hunk scope | jj command |
//! |-----------------|------------|
//! | worktree        | `jj diff --git` (working-copy commit vs parents) |
//! | staged          | empty — jj has no index; stderr note |
//! | working-set     | same as worktree |
//!
//! `include_untracked` is a no-op under jj: new files are typically part of the
//! working-copy commit already (`snapshot.auto-track`). See docs.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

use crate::config::DiffScope;
use crate::ir::FileOrigin;
use crate::source::ProducedDiff;

/// Produce a working-copy / revision review from a jj workspace root.
pub fn jj_diff_produced(
    workspace: &Path,
    scope: DiffScope,
    pathspecs: &[String],
    include_untracked: bool,
) -> Result<ProducedDiff> {
    match scope {
        DiffScope::Staged => {
            eprintln!(
                "note: jj has no staging area; `--staged` / scope=staged yields an empty review. \
                 Use plain `next-hunk diff` for working-copy changes."
            );
            return Ok(ProducedDiff::default());
        }
        DiffScope::Worktree | DiffScope::WorkingSet => {}
    }

    if include_untracked {
        eprintln!(
            "note: `--include-untracked` is ignored in jj workspaces \
             (new files usually appear in `jj diff` via working-copy snapshot)."
        );
    }

    let mut args = vec!["diff".to_string(), "--git".to_string()];
    for p in pathspecs {
        args.push(p.clone());
    }
    let text = run_jj(workspace, &args)?;
    Ok(produced_from_git_diff(text))
}

/// Diff a single revision or a range for `next-hunk show`.
///
/// Accepts:
/// - a jj revset / change id / commit id (e.g. `@`, `@-`, `main@origin`)
/// - git-style `A..B` → `jj diff --from A --to B --git`
/// - git-style `A...B` → merge-base style via `heads(::A & ::B)`
pub fn jj_show(workspace: &Path, rev: &str) -> Result<String> {
    let rev = rev.trim();
    if rev.is_empty() {
        bail!("empty revision");
    }

    if let Some((a, b, merge_base)) = parse_range(rev) {
        let from = if merge_base {
            // Approximate git A...B (diff from merge-base to B).
            format!("heads(::{a} & ::{b})")
        } else {
            a.to_string()
        };
        let args = [
            "diff".to_string(),
            "--git".to_string(),
            "--from".to_string(),
            from,
            "--to".to_string(),
            b.to_string(),
        ];
        return run_jj(workspace, &args);
    }

    // Single revision: changes introduced by that commit vs its parents.
    let args = [
        "diff".to_string(),
        "--git".to_string(),
        "-r".to_string(),
        rev.to_string(),
    ];
    run_jj(workspace, &args)
}

/// True when the `jj` binary is on `PATH`.
pub fn jj_available() -> bool {
    Command::new("jj")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run `jj <args…>` with `-R workspace`, return stdout as UTF-8 text.
fn run_jj(workspace: &Path, args: &[String]) -> Result<String> {
    let mut cmd = Command::new("jj");
    // Color off so the parser sees clean unified-diff text.
    cmd.arg("--no-pager")
        .arg("--color=never")
        .arg("-R")
        .arg(workspace)
        .args(args)
        // Avoid interactive prompts in agent/CI contexts.
        .env("JJ_CONFIG", "") // still loads defaults; empty extra config
        .env("NO_COLOR", "1");

    let output = cmd.output().with_context(|| {
        format!(
            "failed to run `jj` (is it installed and on PATH?). \
                 workspace={}",
            workspace.display()
        )
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else {
            stdout.trim().to_string()
        };
        return Err(anyhow!(
            "jj {} failed (exit {}): {detail}",
            args.join(" "),
            output.status.code().unwrap_or(-1)
        ));
    }

    String::from_utf8(output.stdout).context("jj output is not valid UTF-8")
}

fn produced_from_git_diff(text: String) -> ProducedDiff {
    let origins = text
        .lines()
        .filter(|l| l.starts_with("diff --git "))
        .map(|_| FileOrigin::Modified)
        .collect();
    ProducedDiff { text, origins }
}

/// Parse `A..B` / `A...B`. Returns `(left, right, triple_dot)`.
fn parse_range(rev: &str) -> Option<(&str, &str, bool)> {
    if let Some(i) = rev.find("...") {
        let a = &rev[..i];
        let b = &rev[i + 3..];
        if !a.is_empty() && !b.is_empty() {
            return Some((a, b, true));
        }
    }
    if let Some(i) = rev.find("..") {
        // Avoid matching a lone `..` that is part of `...` (already handled).
        if rev.get(i..i + 3) == Some("...") {
            return None;
        }
        let a = &rev[..i];
        let b = &rev[i + 2..];
        if !a.is_empty() && !b.is_empty() {
            return Some((a, b, false));
        }
    }
    None
}

/// Resolve workspace root via marker walk (caller usually already has it).
#[allow(dead_code)]
pub fn find_jj_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    if dir.is_file() {
        dir = dir.parent()?.to_path_buf();
    }
    loop {
        if dir.join(".jj").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_range_double_dot() {
        let (a, b, triple) = parse_range("main..@").unwrap();
        assert_eq!(a, "main");
        assert_eq!(b, "@");
        assert!(!triple);
    }

    #[test]
    fn parse_range_triple_dot() {
        let (a, b, triple) = parse_range("main...feature").unwrap();
        assert_eq!(a, "main");
        assert_eq!(b, "feature");
        assert!(triple);
    }

    #[test]
    fn parse_range_rejects_empty_sides() {
        assert!(parse_range("..@").is_none());
        assert!(parse_range("main..").is_none());
        assert!(parse_range("main").is_none());
    }

    #[test]
    fn produced_marks_each_git_file_header() {
        let text = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1 +1 @@
-x
+y
";
        let p = produced_from_git_diff(text.into());
        assert_eq!(p.origins.len(), 2);
        assert!(p.origins.iter().all(|o| *o == FileOrigin::Modified));
    }
}
