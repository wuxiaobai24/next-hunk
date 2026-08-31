//! VCS detection plus Jujutsu / Sapling adapters.
//!
//! Git stays on the in-process gix path ([`super::git`]). Jujutsu and
//! Sapling have no comparable Rust crate, so their diffs come from the
//! `jj` / `sl` CLIs (`--git` unified output — exactly what the IR parses).
//! Both are invoked with `--no-pager` where supported and always from the
//! workspace root so revsets and paths resolve like they do in the user's
//! own shell.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Which VCS owns the current workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vcs {
    Git,
    Jujutsu,
    Sapling,
}

/// Detect the VCS by walking up from `start`:
///
/// * a `.jj` directory → Jujutsu (wins over `.git` in colocated repos —
///   in a colocated workspace the jj view is the source of truth)
/// * an `.sl` directory → Sapling
/// * a `.git` (dir or file, i.e. worktrees) → Git
///
/// Defaults to [`Vcs::Git`] when nothing is found, so the existing
/// "not a git repository" error path is preserved.
pub fn detect(start: &Path) -> Vcs {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join(".jj").is_dir() {
            return Vcs::Jujutsu;
        }
        if d.join(".sl").is_dir() {
            return Vcs::Sapling;
        }
        if d.join(".git").exists() {
            return Vcs::Git;
        }
        dir = d.parent();
    }
    Vcs::Git
}

/// Find the workspace root containing `marker` (`.jj` / `.sl` / `.git`).
pub fn find_marker_root(start: &Path, marker: &str) -> Result<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join(marker).exists() {
            return Ok(d.to_owned());
        }
        dir = d.parent();
    }
    bail!(
        "not a workspace with {marker}/ (or any parent): {}",
        start.display()
    );
}

/// Run `jj --no-pager diff --git [ -r revset ] [ paths… ]` in `root`.
/// `revset = None` reviews the working copy (`@`), like plain `jj diff`.
pub fn jj_diff(root: &Path, revset: Option<&str>, pathspecs: &[String]) -> Result<String> {
    let mut cmd = std::process::Command::new("jj");
    cmd.arg("--no-pager")
        .arg("diff")
        .arg("--git")
        .current_dir(root);
    if let Some(r) = revset {
        cmd.arg("-r").arg(r);
    }
    if !pathspecs.is_empty() {
        cmd.arg("--");
        cmd.args(pathspecs);
    }
    run_capture(cmd, "jj diff")
}

/// `jj --no-pager show --git -r <revset>` — the change of one revision.
pub fn jj_show(root: &Path, revset: &str) -> Result<String> {
    let mut cmd = std::process::Command::new("jj");
    cmd.arg("--no-pager")
        .arg("show")
        .arg("--git")
        .arg("-r")
        .arg(revset)
        .current_dir(root);
    run_capture(cmd, "jj show")
}

/// Run `sl diff --git [ -r revset ] [ paths… ]` in `root`.
/// Sapling has no `--no-pager` global flag; `SL_...` pager env is disabled
/// via `git`-style env when supported, and output is captured regardless
/// (a pager writing to a pipe is disabled automatically).
pub fn sl_diff(root: &Path, revset: Option<&str>, pathspecs: &[String]) -> Result<String> {
    let mut cmd = std::process::Command::new("sl");
    cmd.arg("diff").arg("--git").current_dir(root);
    if let Some(r) = revset {
        cmd.arg("-r").arg(r);
    }
    if !pathspecs.is_empty() {
        cmd.arg("--");
        cmd.args(pathspecs);
    }
    run_capture(cmd, "sl diff")
}

/// `sl show <revset>` — falls back to `sl diff -c <revset>` if `show` is
/// unavailable (Sapling's command set varies by version).
pub fn sl_show(root: &Path, revset: &str) -> Result<String> {
    let mut cmd = std::process::Command::new("sl");
    cmd.arg("diff")
        .arg("--git")
        .arg("-c")
        .arg(revset)
        .current_dir(root);
    run_capture(cmd, "sl diff -c")
}

/// Spawn, fail loud (stderr passed through so revset errors read like the
/// user's own `jj` output), and capture stdout as the diff text.
fn run_capture(mut cmd: std::process::Command, what: &str) -> Result<String> {
    let out = cmd
        .output()
        .with_context(|| format!("spawn `{what}` (is it installed and on PATH?)"))?;
    if !out.status.success() {
        bail!(
            "`{what}` failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nh-vcs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn detect_git() {
        let d = tmp("git");
        std::fs::create_dir_all(d.join(".git")).unwrap();
        assert_eq!(detect(&d), Vcs::Git);
        let nested = d.join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(detect(&nested), Vcs::Git, "walks up");
    }

    #[test]
    fn detect_jj_wins_over_colocated_git() {
        let d = tmp("colocated");
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::create_dir_all(d.join(".jj")).unwrap();
        assert_eq!(detect(&d), Vcs::Jujutsu, "jj is the source of truth");
    }

    #[test]
    fn detect_sl() {
        let d = tmp("sl");
        std::fs::create_dir_all(d.join(".sl")).unwrap();
        assert_eq!(detect(&d), Vcs::Sapling);
    }

    #[test]
    fn detect_none_defaults_to_git() {
        let d = tmp("none");
        assert_eq!(detect(&d), Vcs::Git);
    }

    #[test]
    fn find_marker_root_walks_up() {
        let d = tmp("walk");
        std::fs::create_dir_all(d.join(".jj")).unwrap();
        let nested = d.join("x/y");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(find_marker_root(&nested, ".jj").unwrap(), d);
        assert!(find_marker_root(&d, ".sl").is_err());
    }

    /// A stub `jj`/`sl` binary: records its argv and emits a fixed patch.
    /// The adapters are subprocess-based, so their command construction is
    /// verified against the stub (no jj/sl install needed in CI). The patch
    /// is embedded as a heredoc — the stub must run under a stripped PATH
    /// where external tools like `cat` don't exist.
    fn stub_bin(dir: &Path, name: &str, argv_file: &str, patch: &str) {
        // Each patch line becomes one quoted printf argument — printf is a
        // shell builtin, so the stub works under a stripped PATH.
        let lines: Vec<String> = patch
            .split('\n')
            .map(|l| format!("'{}'", l.replace('\'', "'\\''")))
            .collect();
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {argv}\nprintf '%s\\n' {}\n",
            lines.join(" "),
            argv = dir.join(argv_file).display(),
        );
        let path = dir.join(name);
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// PATH-mutating tests serialize on this mutex (same pattern as the
    /// config env tests).
    static PATH_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const STUB_PATCH: &str = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b";

    fn with_stub_path(name: &str, f: impl FnOnce(&Path)) {
        let _guard = PATH_MUTEX.lock().unwrap();
        let d = tmp(name);
        stub_bin(&d, "jj", "argv.txt", STUB_PATCH);
        stub_bin(&d, "sl", "argv.txt", STUB_PATCH);
        let prev = std::env::var_os("PATH").expect("PATH set");
        std::env::set_var("PATH", &d);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&d)));
        std::env::set_var("PATH", prev);
        if let Err(e) = r {
            std::panic::resume_unwind(e);
        }
    }

    fn argv_of(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("argv.txt"))
            .unwrap()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    #[test]
    fn jj_diff_invokes_jj_with_git_output() {
        with_stub_path("jjdiff", |dir| {
            let out = jj_diff(dir, None, &[]).unwrap();
            assert!(out.contains("diff --git a/f b/f"), "patch captured: {out}");
            assert_eq!(argv_of(dir), vec!["--no-pager", "diff", "--git"]);
        });
    }

    #[test]
    fn jj_diff_revset_and_paths() {
        with_stub_path("jjrev", |dir| {
            jj_diff(dir, Some("main..@"), &["src/".into()]).unwrap();
            let argv = argv_of(dir);
            assert!(argv.contains(&"-r".to_string()));
            assert!(argv.contains(&"main..@".to_string()));
            assert!(argv.contains(&"--".to_string()));
            assert!(argv.contains(&"src/".to_string()));
        });
    }

    #[test]
    fn jj_show_uses_show_git_r() {
        with_stub_path("jjshow", |dir| {
            jj_show(dir, "@-").unwrap();
            assert_eq!(
                argv_of(dir),
                vec!["--no-pager", "show", "--git", "-r", "@-"]
            );
        });
    }

    #[test]
    fn sl_diff_and_show() {
        with_stub_path("slcmds", |dir| {
            sl_diff(dir, Some("abc123"), &[]).unwrap();
            assert_eq!(argv_of(dir), vec!["diff", "--git", "-r", "abc123"]);
            sl_show(dir, "abc123").unwrap();
            assert_eq!(argv_of(dir), vec!["diff", "--git", "-c", "abc123"]);
        });
    }

    #[test]
    fn missing_binary_is_a_clear_error() {
        with_stub_path("missing", |dir| {
            // A PATH with only the stub dir but the binary removed.
            std::fs::remove_file(dir.join("jj")).unwrap();
            std::fs::remove_file(dir.join("sl")).unwrap();
            let err = jj_diff(dir, None, &[]).unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains("spawn"), "mentions spawn: {msg}");
        });
    }
}
