//! Terminal-multiplexer overlay launcher (tmux / zellij).
//!
//! `next-hunk overlay` pops a floating review TUI inside the human's mux
//! session, then prints the cached full export JSON on the caller's stdout
//! so agents can parse feedback without owning a TTY.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};

/// Which terminal multiplexer (if any) is hosting the current process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuxKind {
    Tmux,
    Zellij,
    None,
}

/// Detect the active mux from environment variables (testable).
///
/// Preference when both are set: **tmux** (more reliable blocking popup).
pub fn detect_mux_from_env(tmux: Option<&str>, zellij: Option<&str>) -> MuxKind {
    if tmux.map(|s| !s.is_empty()).unwrap_or(false) {
        return MuxKind::Tmux;
    }
    if zellij.map(|s| !s.is_empty()).unwrap_or(false) {
        return MuxKind::Zellij;
    }
    MuxKind::None
}

/// Detect from the real process environment.
pub fn detect_mux() -> MuxKind {
    detect_mux_from_env(
        env::var("TMUX").ok().as_deref(),
        env::var("ZELLIJ").ok().as_deref(),
    )
}

/// Shell-escape a single argument for embedding in `sh -c` / `tmux -E`.
pub fn shell_quote(s: &str) -> String {
    // POSIX single-quote style: 'foo'\''bar'
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Human-facing fallback when neither tmux nor zellij is active.
pub fn fallback_help() -> String {
    r#"error: `overlay` requires a live tmux or zellij session ($TMUX / $ZELLIJ unset)

Fallbacks (pick one):

  1. Open an adjacent pane/window and run a one-shot review there:
       next-hunk diff --all --include-untracked --select --export-on-quit json
     Then recover JSON from this side:
       next-hunk last-export

  2. Persistent session (iterate focus/notes/decision):
       next-hunk serve --all --include-untracked
     Agent: next-hunk diff --focus … --note …
            next-hunk decision / comment / last-export

  3. Enter tmux/zellij first, then:
       next-hunk overlay --all --include-untracked

See skill/next-hunk/SKILL.md § "Overlay (one-shot from agent session)"."#
        .to_string()
}

/// Diff flags forwarded into the floating `next-hunk diff` process.
#[derive(Debug, Clone, Default)]
pub struct OverlayDiffOpts<'a> {
    pub staged: bool,
    pub all: bool,
    pub base: Option<&'a str>,
    pub range: Option<&'a str>,
    pub strategy: Option<&'a str>,
    pub include_untracked: bool,
    pub focus: Option<&'a str>,
    pub notes: &'a [String],
    pub layout: Option<&'a str>,
    pub theme_preset: Option<&'a str>,
    pub vcs: Option<&'a str>,
    pub extra: &'a [String],
}

/// Build the inner `next-hunk diff …` argv (without the binary path).
///
/// Always forces `--select` and `--export-on-quit json` so quit writes a full
/// report the parent can recover via last-export.
pub fn build_diff_argv(opts: &OverlayDiffOpts<'_>) -> Vec<String> {
    let mut args = vec![
        "diff".into(),
        "--select".into(),
        "--export-on-quit".into(),
        "json".into(),
        // Overlay is always a dedicated one-shot; never steal a live serve.
        "--no-forward".into(),
    ];
    if opts.staged {
        args.push("--staged".into());
    }
    if opts.all {
        args.push("--all".into());
    }
    if opts.include_untracked {
        args.push("--include-untracked".into());
    }
    if let Some(b) = opts.base {
        args.push("--base".into());
        args.push(b.into());
    }
    if let Some(r) = opts.range {
        args.push("--range".into());
        args.push(r.into());
    }
    if let Some(s) = opts.strategy {
        args.push("--strategy".into());
        args.push(s.into());
    }
    if let Some(f) = opts.focus {
        args.push("--focus".into());
        args.push(f.into());
    }
    for n in opts.notes {
        args.push("--note".into());
        args.push(n.clone());
    }
    if let Some(l) = opts.layout {
        args.push("--layout".into());
        args.push(l.into());
    }
    if let Some(t) = opts.theme_preset {
        args.push("--theme-preset".into());
        args.push(t.into());
    }
    if let Some(v) = opts.vcs {
        args.push("--vcs".into());
        args.push(v.into());
    }
    for e in opts.extra {
        args.push(e.clone());
    }
    args
}

/// Build a `sh -c` string that cds to `cwd` and runs `exe` with `diff_argv`.
pub fn build_shell_command(exe: &Path, cwd: &Path, diff_argv: &[String]) -> String {
    let mut parts = Vec::with_capacity(diff_argv.len() + 1);
    parts.push(shell_quote(&exe.display().to_string()));
    for a in diff_argv {
        parts.push(shell_quote(a));
    }
    format!(
        "cd {} && exec {}",
        shell_quote(&cwd.display().to_string()),
        parts.join(" ")
    )
}

/// Snapshot of last-export mtime used to detect a fresh write after overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportStamp {
    pub path: Option<PathBuf>,
    pub mtime: Option<SystemTime>,
    pub len: Option<u64>,
}

pub fn stamp_last_export(workdir: &Path) -> ExportStamp {
    let path = crate::tui::persist::resolve_last_export_path(None, Some(workdir));
    let (mtime, len) = path
        .as_ref()
        .and_then(|p| fs::metadata(p).ok())
        .map(|m| (m.modified().ok(), Some(m.len())))
        .unwrap_or((None, None));
    ExportStamp { path, mtime, len }
}

pub fn export_is_fresh(before: &ExportStamp, after: &ExportStamp) -> bool {
    match (&before.path, &after.path) {
        (Some(bp), Some(ap)) if bp == ap => match (before.mtime, after.mtime) {
            (None, Some(_)) => true,
            (Some(b), Some(a)) if a > b => true,
            (Some(_), Some(_)) if before.len != after.len => true,
            _ => false,
        },
        (_, Some(_)) => true,
        _ => false,
    }
}

/// Full overlay session: stamp → launch mux host → require fresh export → print.
pub fn run_overlay_session(
    mux: MuxKind,
    exe: &Path,
    cwd: &Path,
    diff_argv: &[String],
) -> Result<()> {
    if matches!(mux, MuxKind::None) {
        bail!("{}", fallback_help());
    }

    let before = stamp_last_export(cwd);

    match mux {
        MuxKind::Tmux => run_tmux_popup(exe, cwd, diff_argv)?,
        MuxKind::Zellij => run_zellij_float(exe, cwd, diff_argv)?,
        MuxKind::None => unreachable!(),
    }

    let after = stamp_last_export(cwd);
    if !export_is_fresh(&before, &after) {
        bail!(
            "overlay finished without a new export \
             (empty diff, cancelled popup, or review exited without writing). \
             Refusing to print a stale last-export. \
             Try: next-hunk last-export  # only if you know it is current"
        );
    }

    let report = crate::tui::persist::load_last_export(Some(cwd)).ok_or_else(|| {
        anyhow::anyhow!("last-export disappeared after overlay (race or permission error)")
    })?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

fn run_tmux_popup(exe: &Path, cwd: &Path, diff_argv: &[String]) -> Result<()> {
    let shell_cmd = build_shell_command(exe, cwd, diff_argv);
    let status = Command::new("tmux")
        .args(["display-popup", "-w", "90%", "-h", "90%", "-E", &shell_cmd])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("spawn tmux display-popup (is tmux installed and are you inside a session?)")?;

    if !status.success() {
        // Non-zero often means the human closed the popup (Esc) or next-hunk
        // failed. Surface the code; fresh-export check still decides whether
        // we print JSON.
        eprintln!(
            "note: tmux display-popup exited with status {}",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

fn run_zellij_float(exe: &Path, cwd: &Path, diff_argv: &[String]) -> Result<()> {
    // `zellij run` returns immediately; wait on a done-marker the child touches.
    let marker_dir = env::temp_dir();
    let marker = marker_dir.join(format!(
        "next-hunk-overlay-done-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_file(&marker);

    let mut inner_parts = Vec::new();
    inner_parts.push(shell_quote(&exe.display().to_string()));
    for a in diff_argv {
        inner_parts.push(shell_quote(a));
    }
    // Run review, always touch the marker so the parent unblocks.
    let inner = format!(
        "{} ; touch {}",
        inner_parts.join(" "),
        shell_quote(&marker.display().to_string())
    );

    let status = Command::new("zellij")
        .args([
            "run",
            "--floating",
            "--close-on-exit",
            "--width",
            "90%",
            "--height",
            "90%",
            "--cwd",
            &cwd.display().to_string(),
            "--",
            "sh",
            "-c",
            &inner,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .context("spawn zellij run (is zellij installed and are you inside a session?)")?;

    if !status.success() {
        eprintln!(
            "note: zellij run exited with status {}",
            status.code().unwrap_or(-1)
        );
    }

    // Poll for the done marker (human is interacting in the floating pane).
    let deadline = std::time::Instant::now() + Duration::from_secs(60 * 60); // 1h cap
    while !marker.exists() {
        if std::time::Instant::now() > deadline {
            let _ = fs::remove_file(&marker);
            bail!("timed out waiting for zellij overlay pane to finish (1h)");
        }
        thread::sleep(Duration::from_millis(150));
    }
    let _ = fs::remove_file(&marker);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_prefers_tmux_when_both_set() {
        assert_eq!(
            detect_mux_from_env(Some("/tmp/tmux-1000/default,123,0"), Some("0")),
            MuxKind::Tmux
        );
    }

    #[test]
    fn detect_zellij_when_only_zellij() {
        assert_eq!(detect_mux_from_env(None, Some("0")), MuxKind::Zellij);
        assert_eq!(
            detect_mux_from_env(Some(""), Some("session")),
            MuxKind::Zellij
        );
    }

    #[test]
    fn detect_none_when_unset() {
        assert_eq!(detect_mux_from_env(None, None), MuxKind::None);
        assert_eq!(detect_mux_from_env(Some(""), Some("")), MuxKind::None);
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("foo"), "'foo'");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn build_diff_argv_forces_select_and_export() {
        let notes = vec!["banner=hi".into()];
        let args = build_diff_argv(&OverlayDiffOpts {
            all: true,
            include_untracked: true,
            focus: Some("src/a.rs:1"),
            notes: &notes,
            ..Default::default()
        });
        assert!(args.contains(&"--select".into()));
        assert!(args.windows(2).any(|w| w == ["--export-on-quit", "json"]));
        assert!(args.contains(&"--no-forward".into()));
        assert!(args.contains(&"--all".into()));
        assert!(args.contains(&"--include-untracked".into()));
        assert!(args.windows(2).any(|w| w == ["--focus", "src/a.rs:1"]));
        assert!(args.windows(2).any(|w| w == ["--note", "banner=hi"]));
    }

    #[test]
    fn build_shell_command_cds_and_execs() {
        let cmd = build_shell_command(
            Path::new("/usr/bin/next-hunk"),
            Path::new("/work/repo"),
            &["diff".into(), "--select".into()],
        );
        assert!(cmd.starts_with("cd '/work/repo' && exec "));
        assert!(cmd.contains("'/usr/bin/next-hunk'"));
        assert!(cmd.contains("'--select'"));
    }

    #[test]
    fn fallback_help_mentions_tmux_and_serve() {
        let h = fallback_help();
        assert!(h.contains("tmux") || h.contains("zellij"));
        assert!(h.contains("last-export"));
        assert!(h.contains("serve"));
    }

    #[test]
    fn export_freshness_detects_new_mtime() {
        let p = PathBuf::from("/tmp/fake-export.json");
        let before = ExportStamp {
            path: Some(p.clone()),
            mtime: Some(SystemTime::UNIX_EPOCH),
            len: Some(10),
        };
        let after = ExportStamp {
            path: Some(p),
            mtime: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(5)),
            len: Some(10),
        };
        assert!(export_is_fresh(&before, &after));
        assert!(!export_is_fresh(&after, &after));
    }

    #[test]
    fn export_freshness_detects_first_write() {
        let p = PathBuf::from("/tmp/fake-export.json");
        let before = ExportStamp {
            path: Some(p.clone()),
            mtime: None,
            len: None,
        };
        let after = ExportStamp {
            path: Some(p),
            mtime: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
            len: Some(3),
        };
        assert!(export_is_fresh(&before, &after));
    }
}
