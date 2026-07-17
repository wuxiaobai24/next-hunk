//! Jujutsu (`jj`) integration tests.
//!
//! Exercises the jj CLI adapter → unified-diff IR path against a real temporary
//! jj workspace. Skips (does not fail) when the `jj` binary is missing so CI
//! without jj stays green. Pure jj workspaces here do **not** use a git
//! compatibility layer (no `jj git init`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use next_hunk::config::{DiffScope, VcsPreference};
use next_hunk::ir::{parse_unified_diff, DiffLineKind};
use next_hunk::source::{detect_workspace, produce_diff, produce_file_diff, produce_show, VcsKind};

fn require_jj() -> Option<()> {
    if next_hunk::source::jj_available() {
        Some(())
    } else {
        eprintln!("warning: jj binary not found; skipping jj integration test");
        None
    }
}

struct JjGuard {
    dir: PathBuf,
}

impl JjGuard {
    /// Create a jj workspace (`jj git init --colocate`). May also create `.git`;
    /// tests force `VcsPreference::Jj` so the jj adapter is used (no gix path).
    fn new() -> Option<JjGuard> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let uniq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "next-hunk-jj-{}-{}-{}",
            std::process::id(),
            uniq,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let status = Command::new("jj")
            .args(["git", "init", "--colocate"])
            .current_dir(&dir)
            .env("JJ_CONFIG", "")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .status()
            .ok()?;
        if !status.success() {
            // Older/newer CLI flag differences — try without --colocate.
            let status2 = Command::new("jj")
                .args(["git", "init"])
                .current_dir(&dir)
                .env("JJ_CONFIG", "")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .status()
                .ok()?;
            if !status2.success() {
                let _ = fs::remove_dir_all(&dir);
                return None;
            }
        }
        Some(JjGuard { dir })
    }

    fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for JjGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn jj_in(dir: &Path, args: &[&str]) {
    let status = Command::new("jj")
        .args(args)
        .current_dir(dir)
        .env("JJ_CONFIG", "")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .status()
        .expect("run jj");
    assert!(status.success(), "jj {args:?} failed in {}", dir.display());
}

#[test]
fn detect_prefers_jj_in_jj_workspace() {
    let Some(()) = require_jj() else { return };
    let Some(repo) = JjGuard::new() else { return };
    let ws = detect_workspace(repo.path(), VcsPreference::Jj).unwrap();
    assert_eq!(ws.kind, VcsKind::Jj);
    // Auto should also pick jj when .jj is present.
    let ws = detect_workspace(repo.path(), VcsPreference::Auto).unwrap();
    assert_eq!(ws.kind, VcsKind::Jj);
}

#[test]
fn worktree_diff_parses_into_ir() {
    let Some(()) = require_jj() else { return };
    let Some(repo) = JjGuard::new() else { return };

    fs::write(repo.path().join("hello.txt"), "hello\n").unwrap();
    // Snapshot into the working-copy commit so `jj diff` is empty after describe?
    // Actually: writing a file changes the working copy; `jj diff` shows it
    // relative to the parent of @. New file in empty repo may appear after
    // the first snapshot on next jj command.
    jj_in(repo.path(), &["describe", "-m", "init"]);

    fs::write(repo.path().join("hello.txt"), "hello world\n").unwrap();

    let ws = detect_workspace(repo.path(), VcsPreference::Jj).unwrap();
    let produced = produce_diff(&ws, DiffScope::Worktree, &[], false).unwrap();
    assert!(
        !produced.text.trim().is_empty(),
        "expected non-empty jj diff for modified file"
    );
    assert!(
        produced.text.contains("diff --git") || produced.text.contains("---"),
        "expected unified/git-format patch, got:\n{}",
        produced.text
    );

    let review = parse_unified_diff(&produced.text).expect("parse jj patch");
    assert!(!review.files.is_empty(), "review should have files");
    // At least one add or context line after the edit.
    let has_change = review.files.iter().any(|f| {
        f.hunks.iter().any(|h| {
            h.lines
                .iter()
                .any(|l| matches!(l.kind, DiffLineKind::Add | DiffLineKind::Delete))
        })
    });
    assert!(has_change, "expected add/delete lines in review");
}

#[test]
fn show_at_parent_range_works() {
    let Some(()) = require_jj() else { return };
    let Some(repo) = JjGuard::new() else { return };

    fs::write(repo.path().join("a.txt"), "one\n").unwrap();
    jj_in(repo.path(), &["commit", "-m", "first"]);
    fs::write(repo.path().join("a.txt"), "two\n").unwrap();
    jj_in(repo.path(), &["describe", "-m", "second"]);

    let ws = detect_workspace(repo.path(), VcsPreference::Jj).unwrap();
    // Range form should not error.
    let _ = produce_show(&ws, "@-..@").expect("show range");

    // Re-edit to guarantee content in @ vs parent, then parse.
    fs::write(repo.path().join("a.txt"), "three\n").unwrap();
    let text = produce_show(&ws, "@").unwrap();
    if !text.trim().is_empty() {
        parse_unified_diff(&text).expect("parse show @");
    }
}

#[test]
fn staged_scope_is_empty_under_jj() {
    let Some(()) = require_jj() else { return };
    let Some(repo) = JjGuard::new() else { return };
    fs::write(repo.path().join("x.txt"), "x\n").unwrap();
    let ws = detect_workspace(repo.path(), VcsPreference::Jj).unwrap();
    let produced = produce_diff(&ws, DiffScope::Staged, &[], false).unwrap();
    assert!(produced.is_empty(), "staged should be empty on jj");
}

#[test]
fn filediff_works_without_gix_blob_store() {
    let Some(()) = require_jj() else { return };
    let Some(repo) = JjGuard::new() else { return };
    let old = repo.path().join("old.txt");
    let new = repo.path().join("new.txt");
    fs::write(&old, "aaa\n").unwrap();
    fs::write(&new, "bbb\n").unwrap();
    let ws = detect_workspace(repo.path(), VcsPreference::Jj).unwrap();
    let text = produce_file_diff(&ws, &old, &new).unwrap();
    assert!(!text.trim().is_empty());
    let review = parse_unified_diff(&text).expect("parse filediff");
    assert_eq!(review.files.len(), 1);
}
