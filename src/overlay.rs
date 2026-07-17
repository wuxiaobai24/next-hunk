//! Terminal overlay launcher for agent-session review (WXB-24).
//!
//! Detects the host multiplexer (`$TMUX` / `$ZELLIJ`), opens a floating
//! review TUI that does not leave the agent session, then returns the
//! quit-time export JSON on the caller's stdout.
//!
//! Design notes (vs a plain `diff --select`):
//! - Popup TUI stdout is owned by the multiplexer client, not the agent
//!   process. The child therefore writes the full report to a temp path via
//!   `NEXT_HUNK_EXPORT_PATH` (also cached as `last-export`).
//! - No multiplexer: clear degradation message (adjacent pane / `serve` /
//!   one-shot in a TTY). Direct one-shot when the caller already has a TTY.

use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// Env var the TUI child honours: when set, quit export is also written here
/// (JSON, full `ReviewReport` shape). Overlay sets this so the parent can
/// read the report after the popup closes.
pub const EXPORT_PATH_ENV: &str = "NEXT_HUNK_EXPORT_PATH";

/// Which host surface will show the review TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayBackend {
    /// `tmux display-popup -E` (blocks until quit).
    Tmux,
    /// `zellij run --floating` + sentinel wait.
    Zellij,
    /// No multiplexer; run one-shot in the current TTY (if any).
    Direct,
}

/// Snapshot of the process environment used for backend selection.
#[derive(Debug, Clone)]
pub struct OverlayEnv {
    pub tmux: bool,
    pub zellij: bool,
    pub stdout_is_tty: bool,
    pub has_tmux_bin: bool,
    pub has_zellij_bin: bool,
}

impl OverlayEnv {
    /// Probe the real process environment and PATH.
    pub fn detect() -> Self {
        Self {
            tmux: env_nonempty("TMUX"),
            zellij: env_nonempty("ZELLIJ") || env_nonempty("ZELLIJ_SESSION_NAME"),
            stdout_is_tty: io::stdout().is_terminal(),
            has_tmux_bin: which("tmux"),
            has_zellij_bin: which("zellij"),
        }
    }

    /// Prefer tmux when both are set (nested setups are rare; tmux popup is
    /// the better blocking capture surface).
    pub fn select_backend(&self) -> Option<OverlayBackend> {
        if self.tmux && self.has_tmux_bin {
            return Some(OverlayBackend::Tmux);
        }
        if self.zellij && self.has_zellij_bin {
            return Some(OverlayBackend::Zellij);
        }
        if self.stdout_is_tty {
            return Some(OverlayBackend::Direct);
        }
        None
    }
}

fn env_nonempty(key: &str) -> bool {
    env::var_os(key).is_some_and(|v| !v.is_empty())
}

fn which(bin: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {bin} >/dev/null 2>&1")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Human-readable degradation when no overlay backend is available.
pub fn degradation_message() -> String {
    r#"next-hunk overlay: no terminal multiplexer detected (need $TMUX or $ZELLIJ).

Cannot open an in-session review overlay from this non-interactive context.

Fallback options (pick one):

  1. tmux  — start (or attach) a tmux session, then re-run:
       next-hunk overlay --all --include-untracked

  2. zellij — same idea inside a zellij session:
       next-hunk overlay --all --include-untracked

  3. Adjacent pane / window (persistent review):
       next-hunk serve --all --include-untracked
     then the agent uses:
       next-hunk diff --focus … --note …
       next-hunk decision / last-export

  4. One-shot in a visible TTY (you are already at a terminal):
       next-hunk diff --all --include-untracked --select --export-on-quit json

Stdout contract on success is the same full export JSON as
`diff --select --export-on-quit json` / `last-export` (schema_version, decisions,
comments, notes, banner)."#
        .to_string()
}

/// Arguments that build the inner `next-hunk diff …` invocation.
#[derive(Debug, Clone, Default)]
pub struct OverlayDiffArgs {
    pub staged: bool,
    pub all: bool,
    pub base: Option<String>,
    pub range: Option<String>,
    pub strategy: Option<String>,
    pub include_untracked: bool,
    pub focus: Option<String>,
    pub note: Vec<String>,
    pub layout: Option<String>,
    pub theme_preset: Option<String>,
    pub vcs: Option<String>,
    pub no_highlight: bool,
    pub no_persist: bool,
    pub extra: Vec<String>,
    /// Override export mode (default `json`).
    pub export_on_quit: Option<String>,
    /// Path to the next-hunk binary (default: current exe).
    pub binary: Option<PathBuf>,
    /// Working directory for the review (default: cwd).
    pub cwd: Option<PathBuf>,
    /// Popup size for tmux/zellij (default 90%).
    pub popup_width: Option<String>,
    pub popup_height: Option<String>,
}

impl OverlayDiffArgs {
    /// Build argv for `next-hunk diff --select --export-on-quit <mode> …`.
    pub fn to_diff_argv(&self) -> Vec<String> {
        let mut args = vec!["diff".to_string(), "--select".to_string()];
        let export = self
            .export_on_quit
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("json");
        args.push("--export-on-quit".into());
        args.push(export.into());
        if self.staged {
            args.push("--staged".into());
        }
        if self.all {
            args.push("--all".into());
        }
        if let Some(ref b) = self.base {
            args.push("--base".into());
            args.push(b.clone());
        }
        if let Some(ref r) = self.range {
            args.push("--range".into());
            args.push(r.clone());
        }
        if let Some(ref s) = self.strategy {
            args.push("--strategy".into());
            args.push(s.clone());
        }
        if self.include_untracked {
            args.push("--include-untracked".into());
        }
        if let Some(ref f) = self.focus {
            args.push("--focus".into());
            args.push(f.clone());
        }
        for n in &self.note {
            args.push("--note".into());
            args.push(n.clone());
        }
        if let Some(ref l) = self.layout {
            args.push("--layout".into());
            args.push(l.clone());
        }
        if let Some(ref t) = self.theme_preset {
            args.push("--theme-preset".into());
            args.push(t.clone());
        }
        if let Some(ref v) = self.vcs {
            args.push("--vcs".into());
            args.push(v.clone());
        }
        if self.no_highlight {
            args.push("--no-highlight".into());
        }
        if self.no_persist {
            args.push("--no-persist".into());
        }
        // Never auto-forward into an existing serve — overlay owns this review.
        args.push("--no-forward".into());
        args.extend(self.extra.iter().cloned());
        args
    }
}

/// Shell-quote a single argument for embedding in `sh -c` strings.
pub fn shell_quote(s: &str) -> String {
    // Single-quote with POSIX escaping: ' → '\''
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

/// Resolve the binary path used to re-exec next-hunk inside the popup.
pub fn resolve_binary(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    env::current_exe().context("resolve next-hunk binary path")
}

/// Run the overlay flow and write the export JSON to `out` (usually stdout).
pub fn run_overlay(args: &OverlayDiffArgs, env: &OverlayEnv) -> Result<()> {
    let backend = match env.select_backend() {
        Some(b) => b,
        None => bail!("{}", degradation_message()),
    };

    let binary = resolve_binary(args.binary.as_deref())?;
    let cwd = match &args.cwd {
        Some(c) => c.clone(),
        None => env::current_dir().context("current_dir")?,
    };
    let export_path = make_export_tempfile()?;
    // Best-effort cleanup of the temp file; report body is already printed.
    let _guard = TempPathGuard(export_path.clone());

    let popup_w = args
        .popup_width
        .clone()
        .or_else(|| env::var("NEXT_HUNK_POPUP_WIDTH").ok())
        .unwrap_or_else(|| "90%".into());
    let popup_h = args
        .popup_height
        .clone()
        .or_else(|| env::var("NEXT_HUNK_POPUP_HEIGHT").ok())
        .unwrap_or_else(|| "90%".into());

    let status = match backend {
        OverlayBackend::Tmux => launch_tmux(&binary, &cwd, args, &export_path, &popup_w, &popup_h)?,
        OverlayBackend::Zellij => {
            launch_zellij(&binary, &cwd, args, &export_path, &popup_w, &popup_h)?
        }
        OverlayBackend::Direct => launch_direct(&binary, &cwd, args, &export_path)?,
    };

    if !status.success() {
        let code = status.code().unwrap_or(1);
        // Still try to print export if the human quit after saving (e.g. non-zero
        // from a wrapper). Prefer a partial report over silence.
        if export_path.is_file()
            && fs::metadata(&export_path)
                .map(|m| m.len() > 0)
                .unwrap_or(false)
        {
            print_export_file(&export_path)?;
            bail!("overlay review exited with status {code} (export still printed above)");
        }
        bail!("overlay review exited with status {code}");
    }

    if !export_path.is_file() {
        bail!(
            "overlay finished but no export was written to {} \
             (did the TUI quit without --export-on-quit?)",
            export_path.display()
        );
    }
    print_export_file(&export_path)?;
    Ok(())
}

struct TempPathGuard(PathBuf);

impl Drop for TempPathGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn make_export_tempfile() -> Result<PathBuf> {
    let dir = env::temp_dir();
    let name = format!(
        "next-hunk-overlay-export-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let path = dir.join(name);
    // Create empty so existence checks are stable; child truncates/writes.
    fs::File::create(&path).with_context(|| format!("create {}", path.display()))?;
    Ok(path)
}

fn print_export_file(path: &Path) -> Result<()> {
    let body = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let trimmed = body.trim();
    if trimmed.is_empty() {
        bail!("export file {} is empty", path.display());
    }
    // Ensure a single trailing newline for agent parsers.
    println!("{trimmed}");
    Ok(())
}

fn build_inner_sh_command(binary: &Path, args: &OverlayDiffArgs, export_path: &Path) -> String {
    let mut parts = Vec::new();
    parts.push(format!(
        "export {}={}",
        EXPORT_PATH_ENV,
        shell_quote(&export_path.display().to_string())
    ));
    parts.push(shell_quote(&binary.display().to_string()));
    for a in args.to_diff_argv() {
        parts.push(shell_quote(&a));
    }
    // Join: export …; /path/next-hunk diff …
    // First token is the export assignment; rest is the command.
    let export = parts[0].clone();
    let cmd = parts[1..].join(" ");
    format!("{export}; exec {cmd}")
}

fn launch_tmux(
    binary: &Path,
    cwd: &Path,
    args: &OverlayDiffArgs,
    export_path: &Path,
    popup_w: &str,
    popup_h: &str,
) -> Result<std::process::ExitStatus> {
    let inner = build_inner_sh_command(binary, args, export_path);
    let title = format!(
        "next-hunk: {}",
        cwd.file_name().and_then(|s| s.to_str()).unwrap_or("review")
    );

    let mut cmd = Command::new("tmux");
    cmd.arg("display-popup")
        .arg("-E")
        .arg("-w")
        .arg(popup_w)
        .arg("-h")
        .arg(popup_h)
        .arg("-d")
        .arg(cwd);
    // -T title needs tmux 3.3+; always try — older tmux errors are rare and
    // the user can still fall back. We pass title when version looks fine.
    if tmux_supports_title() {
        cmd.arg("-T").arg(format!(" {title} "));
    }
    cmd.arg("--").arg("sh").arg("-c").arg(&inner);
    // Inherit parent's stdio for the *launcher* (blocks); popup owns its TTY.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = cmd
        .status()
        .context("failed to run `tmux display-popup` (is tmux on PATH?)")?;
    Ok(status)
}

fn tmux_supports_title() -> bool {
    let out = Command::new("tmux").arg("-V").output().ok();
    let Some(out) = out else {
        return false;
    };
    let s = String::from_utf8_lossy(&out.stdout);
    // "tmux 3.4" / "tmux next-3.4"
    let ver = s.split_whitespace().nth(1).unwrap_or("");
    let digits: String = ver
        .chars()
        .map(|c| {
            if c.is_ascii_digit() || c == '.' {
                c
            } else {
                ' '
            }
        })
        .collect();
    let mut parts = digits.split_whitespace().flat_map(|p| p.split('.'));
    let major: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    major > 3 || (major == 3 && minor >= 3)
}

fn launch_zellij(
    binary: &Path,
    cwd: &Path,
    args: &OverlayDiffArgs,
    export_path: &Path,
    popup_w: &str,
    popup_h: &str,
) -> Result<std::process::ExitStatus> {
    // zellij run does not block; use a sentinel file written after the review.
    let tmp = env::temp_dir();
    let sentinel = tmp.join(format!(
        "next-hunk-overlay-done-{}-{}.rc",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_file(&sentinel);

    let launch_script = tmp.join(format!(
        "next-hunk-overlay-launch-{}-{}.sh",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    // Do not use `exec` so we can capture the review exit code into a sentinel.
    let export_line = format!(
        "export {}={}",
        EXPORT_PATH_ENV,
        shell_quote(&export_path.display().to_string())
    );
    let mut cmd_parts = vec![shell_quote(&binary.display().to_string())];
    for a in args.to_diff_argv() {
        cmd_parts.push(shell_quote(&a));
    }
    let cmd_line = cmd_parts.join(" ");
    let script_body = format!(
        "#!/bin/sh\n{export_line}\n{cmd_line}\nrc=$?\nprintf '%s' \"$rc\" > {sent}.tmp && mv -f {sent}.tmp {sent}\nexit \"$rc\"\n",
        sent = shell_quote(&sentinel.display().to_string()),
    );
    fs::write(&launch_script, &script_body)
        .with_context(|| format!("write {}", launch_script.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&launch_script)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&launch_script, perms)?;
    }

    let title = format!(
        "next-hunk: {}",
        cwd.file_name().and_then(|s| s.to_str()).unwrap_or("review")
    );
    let mut cmd = Command::new("zellij");
    cmd.arg("run")
        .arg("--floating")
        .arg("--close-on-exit")
        .arg("--width")
        .arg(popup_w)
        .arg("--height")
        .arg(popup_h)
        .arg("--name")
        .arg(&title)
        .arg("--cwd")
        .arg(cwd)
        .arg("--")
        .arg(&launch_script);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = cmd
        .status()
        .context("failed to run `zellij run` (is zellij on PATH?)")?;
    if !status.success() {
        let _ = fs::remove_file(&launch_script);
        return Ok(status);
    }

    // Poll sentinel (human may take a long time).
    let deadline = std::time::Instant::now() + Duration::from_secs(60 * 60 * 6); // 6h cap
    while !sentinel.is_file() {
        if std::time::Instant::now() > deadline {
            let _ = fs::remove_file(&launch_script);
            bail!("timed out waiting for zellij overlay to finish");
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let rc_str = fs::read_to_string(&sentinel).unwrap_or_else(|_| "1".into());
    let rc: i32 = rc_str.trim().parse().unwrap_or(1);
    let _ = fs::remove_file(&sentinel);
    let _ = fs::remove_file(&launch_script);
    Ok(exit_status_from_code(rc))
}

fn launch_direct(
    binary: &Path,
    cwd: &Path,
    args: &OverlayDiffArgs,
    export_path: &Path,
) -> Result<std::process::ExitStatus> {
    let mut cmd = Command::new(binary);
    cmd.args(args.to_diff_argv())
        .current_dir(cwd)
        .env(EXPORT_PATH_ENV, export_path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let status = cmd.status().context("failed to run next-hunk diff")?;
    Ok(status)
}

#[cfg(unix)]
fn exit_status_from_code(code: i32) -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(if code == 0 { 0 } else { (code & 0xff) << 8 })
}

#[cfg(not(unix))]
fn exit_status_from_code(code: i32) -> std::process::ExitStatus {
    // Best-effort: spawn a no-op and map — rare on non-unix builds.
    let _ = code;
    Command::new("true")
        .status()
        .unwrap_or_else(|_| Command::new("sh").arg("-c").arg("exit 1").status().unwrap())
}

/// Optional helper used by unit tests / diagnostics.
pub fn write_degradation_to(w: &mut dyn Write) -> io::Result<()> {
    writeln!(w, "{}", degradation_message())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("foo"), "'foo'");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn diff_argv_forces_select_and_export_json() {
        let args = OverlayDiffArgs {
            all: true,
            include_untracked: true,
            focus: Some("src/a.rs:1".into()),
            note: vec!["banner=hi".into()],
            ..Default::default()
        };
        let argv = args.to_diff_argv();
        assert_eq!(argv[0], "diff");
        assert!(argv.contains(&"--select".into()));
        assert!(argv.contains(&"--export-on-quit".into()));
        assert!(argv.contains(&"json".into()));
        assert!(argv.contains(&"--all".into()));
        assert!(argv.contains(&"--include-untracked".into()));
        assert!(argv.contains(&"--no-forward".into()));
        assert!(argv.contains(&"--focus".into()));
        assert!(argv.contains(&"src/a.rs:1".into()));
        assert!(argv.contains(&"--note".into()));
        assert!(argv.contains(&"banner=hi".into()));
    }

    #[test]
    fn select_backend_prefers_tmux() {
        let env = OverlayEnv {
            tmux: true,
            zellij: true,
            stdout_is_tty: false,
            has_tmux_bin: true,
            has_zellij_bin: true,
        };
        assert_eq!(env.select_backend(), Some(OverlayBackend::Tmux));
    }

    #[test]
    fn select_backend_zellij_when_no_tmux() {
        let env = OverlayEnv {
            tmux: false,
            zellij: true,
            stdout_is_tty: false,
            has_tmux_bin: false,
            has_zellij_bin: true,
        };
        assert_eq!(env.select_backend(), Some(OverlayBackend::Zellij));
    }

    #[test]
    fn select_backend_direct_on_tty() {
        let env = OverlayEnv {
            tmux: false,
            zellij: false,
            stdout_is_tty: true,
            has_tmux_bin: false,
            has_zellij_bin: false,
        };
        assert_eq!(env.select_backend(), Some(OverlayBackend::Direct));
    }

    #[test]
    fn select_backend_none_headless() {
        let env = OverlayEnv {
            tmux: false,
            zellij: false,
            stdout_is_tty: false,
            has_tmux_bin: true,
            has_zellij_bin: true,
        };
        assert_eq!(env.select_backend(), None);
    }

    #[test]
    fn degradation_mentions_serve_and_select() {
        let msg = degradation_message();
        assert!(msg.contains("serve"));
        assert!(msg.contains("--select"));
        assert!(msg.contains("TMUX") || msg.contains("tmux"));
        assert!(msg.contains("zellij") || msg.contains("ZELLIJ"));
    }

    #[test]
    fn export_path_env_constant() {
        assert_eq!(EXPORT_PATH_ENV, "NEXT_HUNK_EXPORT_PATH");
    }
}
