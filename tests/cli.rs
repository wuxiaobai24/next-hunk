//! CLI smoke tests: invoke the compiled `next-hunk` binary on fixtures and
//! stdin, asserting exit codes and `inspect` output. Because these pipe stdin
//! / stdout (no tty), the TUI falls back to the inspect summary — which is
//! exactly the path we want to keep healthy.

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn bin() -> PathBuf {
    // CARGO_BIN_EXE_<name> is set by `cargo test` for the bin target.
    PathBuf::from(env!("CARGO_BIN_EXE_next-hunk"))
}

fn fixture(name: &str) -> String {
    let path = PathBuf::from("fixtures").join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
}

#[test]
fn inspect_on_fixture_succeeds() {
    let out = Command::new(bin())
        .args(["inspect", "fixtures/tiny_simple.patch"])
        .output()
        .expect("run next-hunk");
    assert!(
        out.status.success(),
        "exit non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("files=2"), "inspect summary: {stdout}");
    assert!(stdout.contains("stream_rows="));
    assert!(stdout.contains("src/a.rs"));
    assert!(stdout.contains("src/b.rs"));
}

#[test]
fn inspect_empty_stdin_reports_zero() {
    let out = Command::new(bin())
        .args(["inspect", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut child = out;
    {
        let mut stdin = child.stdin.take().expect("stdin");
        use std::io::Write;
        stdin.write_all(b"   \n").unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("files=0"),
        "empty diff should report files=0: {stdout}"
    );
}

#[test]
fn patch_stdin_falls_back_to_inspect() {
    // Non-tty stdout → TUI errors out → fallback prints inspect summary.
    let patch = fixture("tiny_simple.patch");
    let out = Command::new(bin())
        .args(["patch", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut child = out;
    {
        // Write stdin, then drop the ChildStdin to close the pipe. Closing
        // before wait is required so the child sees EOF; on Windows a still-
        // open write end deadlocks the child's read_to_string indefinitely.
        let mut stdin = child.stdin.take().expect("stdin");
        use std::io::Write;
        stdin.write_all(patch.as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "patch - should exit 0 even on non-tty"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // fallback inspect summary should appear
    assert!(
        combined.contains("files=2"),
        "expected inspect fallback: {combined}"
    );
}

#[test]
fn patch_missing_file_errors() {
    let out = Command::new(bin())
        .args(["patch", "does-not-exist.patch"])
        .output()
        .expect("run next-hunk");
    assert!(!out.status.success(), "should fail on missing file");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not found"),
        "stderr should mention not found: {stderr}"
    );
}

#[test]
fn no_subcommand_defaults_to_diff() {
    // In a non-git directory, `diff` (the default) should fail with a clear
    // error rather than panic. Run from /tmp to ensure no enclosing repo.
    let tmp = std::env::temp_dir();
    let out = Command::new(bin())
        .current_dir(&tmp)
        .output()
        .expect("run next-hunk");
    // Either it errors (no repo) — both are acceptable; it must not panic/hang.
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Acceptable: a git-repo-not-found error.
    assert!(
        stderr.contains("not a git repository") || stderr.contains("error") || out.status.success(),
        "default diff should fail gracefully, stderr: {stderr}"
    );
}

// `list` is only implemented under serve+unix; elsewhere it stubs with a
// feature message and never reaches workspace discovery.
#[cfg(all(unix, feature = "serve"))]
#[test]
fn list_all_worktrees_outside_repo_matches_diff_phrasing() {
    // Outside a workspace, repo-aware commands share one clean message.
    // `list --all-worktrees` must not leak gix discovery text.
    let tmp = std::env::temp_dir();
    let list = Command::new(bin())
        .args(["list", "--all-worktrees"])
        .current_dir(&tmp)
        .output()
        .expect("run next-hunk list");
    let diff = Command::new(bin())
        .args(["diff"])
        .current_dir(&tmp)
        .output()
        .expect("run next-hunk diff");
    assert!(
        !list.status.success(),
        "list --all-worktrees should fail outside a repo"
    );
    let list_err = String::from_utf8_lossy(&list.stderr);
    let diff_err = String::from_utf8_lossy(&diff.stderr);
    assert!(
        list_err.contains("not a git or jj workspace"),
        "list --all-worktrees should use the shared workspace message, stderr: {list_err}"
    );
    assert!(
        !list_err.contains("Could not find a git repository"),
        "list --all-worktrees must not leak gix phrasing, stderr: {list_err}"
    );
    assert!(
        diff_err.contains("not a git or jj workspace"),
        "diff baseline should use the shared message, stderr: {diff_err}"
    );
}

#[test]
fn watch_flag_is_recognized() {
    // `--watch` must be a valid flag (clap parses it). In a non-git dir it
    // should fail with the repo error, not an "unknown argument" error —
    // proving the flag exists in the CLI surface.
    let tmp = std::env::temp_dir();
    let out = Command::new(bin())
        .args(["diff", "--watch"])
        .current_dir(&tmp)
        .output()
        .expect("run next-hunk");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("unknown"),
        "--watch should be a recognized flag, stderr: {stderr}"
    );
}

#[test]
fn no_highlight_flag_is_recognized() {
    // `--no-highlight` must be a valid flag (clap parses it). Same rationale
    // as the watch test: existence in the CLI surface.
    let tmp = std::env::temp_dir();
    let out = Command::new(bin())
        .args(["diff", "--no-highlight"])
        .current_dir(&tmp)
        .output()
        .expect("run next-hunk");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("unknown"),
        "--no-highlight should be a recognized flag, stderr: {stderr}"
    );
}

#[test]
fn base_range_strategy_flags_are_recognized() {
    let tmp = std::env::temp_dir();
    for args in [
        vec!["diff", "--base", "origin/main"],
        vec!["diff", "--range", "main..HEAD"],
        vec!["diff", "--strategy", "upstream-ahead"],
        vec!["diff", "--strategy", "merge-base", "--base", "main"],
        vec!["inspect", "--base", "HEAD"],
    ] {
        let out = Command::new(bin())
            .args(&args)
            .current_dir(&tmp)
            .output()
            .expect("run next-hunk");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("unexpected argument") && !stderr.contains("unknown"),
            "flags {:?} should be recognized, stderr: {stderr}",
            args
        );
    }
}

#[test]
fn merge_base_strategy_without_base_errors() {
    // Needs a git repo so we get past discover; then strategy validation fires.
    // Use a temp git repo if available; otherwise skip.
    let tmp = std::env::temp_dir().join(format!("next-hunk-cli-mb-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let init = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&tmp)
        .status();
    if !init.map(|s| s.success()).unwrap_or(false) {
        let _ = std::fs::remove_dir_all(&tmp);
        return;
    }
    let out = Command::new(bin())
        .args(["diff", "--strategy", "merge-base"])
        .current_dir(&tmp)
        .output()
        .expect("run next-hunk");
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(
        !out.status.success(),
        "merge-base without --base should fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("merge-base") && stderr.contains("--base"),
        "expected merge-base/--base guidance, got: {stderr}"
    );
}

#[test]
fn focus_flag_is_recognized() {
    // `--focus` must be a valid flag taking a value. In a non-git dir it fails
    // with the repo error, not an "unknown argument" error.
    let tmp = std::env::temp_dir();
    let out = Command::new(bin())
        .args(["diff", "--focus", "src/a.rs"])
        .current_dir(&tmp)
        .output()
        .expect("run next-hunk");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("unknown"),
        "--focus should be a recognized flag, stderr: {stderr}"
    );
}

#[test]
fn note_flag_is_repeatable() {
    // Multiple `--note` flags must parse without error (clap Append action).
    let tmp = std::env::temp_dir();
    let out = Command::new(bin())
        .args([
            "diff",
            "--note",
            "src/a.rs:1=first",
            "--note",
            "banner=second",
        ])
        .current_dir(&tmp)
        .output()
        .expect("run next-hunk");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("unknown"),
        "--note should be a recognized repeatable flag, stderr: {stderr}"
    );
}

#[test]
fn select_flag_is_recognized() {
    // `--select` must be a valid flag. It will later be tested for the non-tty
    // error path, but first confirm clap accepts it.
    let tmp = std::env::temp_dir();
    let out = Command::new(bin())
        .args(["diff", "--select"])
        .current_dir(&tmp)
        .output()
        .expect("run next-hunk");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("unknown"),
        "--select should be a recognized flag, stderr: {stderr}"
    );
}

#[test]
fn select_in_non_tty_errors_with_clear_message() {
    // `--select` requires an interactive terminal. In a piped (non-tty) child
    // it must fail fast with a message mentioning the tty requirement, so an
    // agent scripting it gets an unambiguous signal.
    let tmp = std::env::temp_dir();
    let out = Command::new(bin())
        .args(["diff", "--select"])
        .current_dir(&tmp)
        .output()
        .expect("run next-hunk");
    assert!(
        !out.status.success(),
        "--select in a non-tty should exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--select") && stderr.contains("interactive"),
        "--select non-tty error should mention --select and interactive, got: {stderr}"
    );
}

#[test]
fn focus_parse_variants_are_accepted() {
    // The three focus spec shapes should all be accepted by clap (no parse
    // error from clap itself). They'll fail at the git step in a temp dir, but
    // the failure must NOT be a "couldn't parse --focus" error.
    let tmp = std::env::temp_dir();
    for spec in &["src/a.rs", "src/a.rs:42", "src/a.rs:h3"] {
        let out = Command::new(bin())
            .args(["diff", "--focus", spec])
            .current_dir(&tmp)
            .output()
            .expect("run next-hunk");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("unexpected argument") && !stderr.contains("invalid"),
            "--focus {spec} should parse cleanly, stderr: {stderr}"
        );
    }
}

#[test]
fn bad_focus_line_number_errors() {
    // A non-numeric line suffix should produce a clear parse error before any
    // git access, so the agent gets actionable feedback.
    let tmp = std::env::temp_dir();
    let out = Command::new(bin())
        .args(["diff", "--focus", "src/a.rs:abc"])
        .current_dir(&tmp)
        .output()
        .expect("run next-hunk");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--focus") && stderr.contains("invalid line"),
        "bad focus line should error clearly, got: {stderr}"
    );
}

#[test]
fn note_missing_equals_errors() {
    // A --note without `=text` is malformed and should fail with guidance.
    let tmp = std::env::temp_dir();
    let out = Command::new(bin())
        .args(["diff", "--note", "src/a.rs:1"])
        .current_dir(&tmp)
        .output()
        .expect("run next-hunk");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--note"),
        "malformed --note should error mentioning --note, got: {stderr}"
    );
}

#[test]
fn pager_empty_stdin_exits_clean() {
    // As git's pager, empty stdin (no diff) must be a clean no-op, exit 0.
    let out = Command::new(bin())
        .args(["pager"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut child = out;
    {
        let mut stdin = child.stdin.take().expect("stdin");
        use std::io::Write;
        stdin.write_all(b"   \n").unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "empty pager stdin should exit 0");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !combined.contains("nothing to review") && !combined.contains("error"),
        "empty pager should be silent, got: {combined}"
    );
}

#[test]
fn pager_reads_stdin_and_renders() {
    // Non-tty stdout → TUI falls back to inspect summary. `pager` must behave
    // like `patch -`: feed a real patch, get the inspect fallback back.
    let patch = fixture("tiny_simple.patch");
    let out = Command::new(bin())
        .args(["pager"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut child = out;
    {
        // Close stdin before waiting (see patch_stdin_falls_back_to_inspect):
        // on Windows the child's read_to_string blocks until EOF, so an
        // un-dropped write end deadlocks. This is the test that hung Windows
        // CI (6h+) before this explicit close was added.
        let mut stdin = child.stdin.take().expect("stdin");
        use std::io::Write;
        stdin.write_all(patch.as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "pager should exit 0 on non-tty");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("files=2"),
        "pager should render the patch (inspect fallback): {combined}"
    );
}

#[test]
fn show_patch_filediff_accept_focus_note_select() {
    // Agent-bridge flags must be clap-valid on show/patch/filediff (aligned
    // with diff). Failure must not be "unexpected argument".
    let tmp = std::env::temp_dir();
    for (cmd, extra) in [
        (vec!["show", "HEAD"], vec![] as Vec<&str>),
        (vec!["patch", "fixtures/tiny_simple.patch"], vec![]),
        // filediff needs two paths; use the same fixture twice so clap gets
        // past arg parsing even if later git-diff fails.
        (
            vec![
                "filediff",
                "fixtures/tiny_simple.patch",
                "fixtures/tiny_edge.patch",
            ],
            vec![],
        ),
    ] {
        let mut args: Vec<&str> = cmd.clone();
        args.extend(["--focus", "src/a.rs", "--note", "banner=hi", "--select"]);
        args.extend(extra);
        let out = Command::new(bin())
            .args(&args)
            .current_dir(if cmd[0] == "show" {
                tmp.as_path()
            } else {
                std::path::Path::new(".")
            })
            .output()
            .expect("run next-hunk");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("unexpected argument") && !stderr.contains("unrecognized"),
            "{} should accept --focus/--note/--select, stderr: {stderr}",
            cmd[0]
        );
    }
}

#[test]
fn focus_and_note_in_non_tty_error_not_silent() {
    // Non-TTY + --focus/--note must exit non-zero — never fall back to inspect
    // while dropping agent annotations (WXB-15).
    let patch = fixture("tiny_simple.patch");
    for args in [
        vec!["patch", "-", "--focus", "src/a.rs"],
        vec!["patch", "-", "--note", "banner=agent note"],
        vec![
            "patch",
            "-",
            "--focus",
            "src/a.rs",
            "--note",
            "src/a.rs:1=why",
        ],
    ] {
        let mut child = Command::new(bin())
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn");
        {
            use std::io::Write;
            let mut stdin = child.stdin.take().expect("stdin");
            stdin.write_all(patch.as_bytes()).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert!(
            !out.status.success(),
            "non-tty {:?} should exit non-zero (got success)",
            args
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("interactive") || stderr.contains("tty"),
            "non-tty {:?} should mention interactive/tty, got: {stderr}",
            args
        );
        // Must not pretend success with an inspect summary on stdout alone.
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            !stdout.starts_with("files="),
            "must not silently print inspect summary when focus/note given: {stdout}"
        );
    }
}

#[test]
fn export_on_quit_json_non_tty_emits_report_not_inspect() {
    // WXB-31: non-TTY + --export-on-quit must emit the agent report, never the
    // inspect summary. Agents pipe stdout and parse JSON.
    let patch = fixture("tiny_simple.patch");
    let mut child = Command::new(bin())
        .args([
            "patch",
            "-",
            "--export-on-quit",
            "json",
            "--note",
            "banner=please review",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(patch.as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "export-on-quit non-tty should exit 0: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.starts_with("files="),
        "must not print inspect summary when export-on-quit set: {stdout}"
    );
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout should be one JSON object");
    assert_eq!(
        v.get("schema_version").and_then(|n| n.as_u64()),
        Some(1),
        "full export must carry schema_version: {v}"
    );
    assert!(v.get("accepted").is_some(), "missing accepted: {v}");
    assert!(v.get("rejected").is_some(), "missing rejected: {v}");
    assert!(v.get("undecided").is_some(), "missing undecided: {v}");
    let undecided = v["undecided"].as_array().expect("undecided array");
    assert!(!undecided.is_empty(), "fixture has hunks: {v}");
    assert_eq!(
        v.get("banner").and_then(|b| b.as_str()),
        Some("please review")
    );
}

#[test]
fn export_on_quit_markdown_non_tty_emits_report() {
    let patch = fixture("tiny_simple.patch");
    let mut child = Command::new(bin())
        .args(["patch", "-", "--export-on-quit", "markdown"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(patch.as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "export-on-quit markdown non-tty should exit 0: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.starts_with("files="),
        "must not print inspect summary: {stdout}"
    );
    assert!(
        stdout.contains("# next-hunk review report"),
        "expected markdown report: {stdout}"
    );
    assert!(
        stdout.contains("## Decisions"),
        "expected decisions section: {stdout}"
    );
}

#[test]
fn export_on_quit_both_non_tty_emits_json_then_markdown() {
    let patch = fixture("tiny_simple.patch");
    let mut child = Command::new(bin())
        .args(["patch", "-", "--export-on-quit", "both"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(patch.as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.starts_with("files="), "no inspect: {stdout}");
    // First non-empty line is JSON; body contains markdown.
    let first_line = stdout.lines().next().expect("output");
    let v: serde_json::Value = serde_json::from_str(first_line).expect("first line should be JSON");
    assert!(v.get("undecided").is_some());
    assert!(
        stdout.contains("# next-hunk review report"),
        "both should include markdown after JSON: {stdout}"
    );
}

#[test]
fn export_on_quit_json_caches_last_export() {
    // Headless export should also write last-export for agents that miss stdout.
    let tmp = std::env::temp_dir().join(format!(
        "next-hunk-cli-lastexport-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let git_ok = Command::new("git")
        .args(["init"])
        .current_dir(&tmp)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !git_ok {
        let _ = std::fs::remove_dir_all(&tmp);
        return;
    }

    let patch = fixture("tiny_simple.patch");
    let mut child = Command::new(bin())
        .args([
            "patch",
            "-",
            "--export-on-quit",
            "json",
            "--note",
            "banner=cached",
        ])
        .current_dir(&tmp)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(patch.as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "export should succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let cached = Command::new(bin())
        .args(["last-export"])
        .current_dir(&tmp)
        .output()
        .expect("last-export");
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(
        cached.status.success(),
        "last-export should recover report: {}",
        String::from_utf8_lossy(&cached.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&cached.stdout).trim())
        .expect("last-export JSON");
    assert_eq!(v.get("schema_version").and_then(|n| n.as_u64()), Some(1));
    assert_eq!(v.get("banner").and_then(|b| b.as_str()), Some("cached"));
    assert!(v.get("undecided").is_some());
}

#[test]
fn pager_garbage_input_exits_nonzero() {
    // Dogfood P1: `echo hello | next-hunk pager` must not exit 0 after a
    // parse failure (agents / scripts treat 0 as success).
    let mut child = Command::new(bin())
        .args(["pager"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(b"hello\n").unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        !out.status.success(),
        "pager garbage input must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("parse") || stderr.contains("error"),
        "pager should report parse/fatal on stderr, got: {stderr}"
    );
    // WXB-34: non-empty garbage must not claim "empty diff input".
    assert!(
        !stderr.contains("empty diff input"),
        "non-empty garbage must not say empty diff input: {stderr}"
    );
    assert!(
        stderr.contains("no valid") || stderr.contains("@@"),
        "should name missing hunk header: {stderr}"
    );
}

#[test]
fn illegal_project_config_fails_diff() {
    // Dogfood P1: `layout = "sidebyside"` must fail startup with field + enums.
    let tmp = std::env::temp_dir().join(format!(
        "next-hunk-cli-badcfg-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(tmp.join(".next-hunk")).unwrap();
    std::fs::write(
        tmp.join(".next-hunk/config.toml"),
        "layout = \"sidebyside\"\n",
    )
    .unwrap();
    // init a git repo so `diff` gets past repo discovery after config load.
    let git_ok = Command::new("git")
        .args(["init"])
        .current_dir(&tmp)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !git_ok {
        let _ = std::fs::remove_dir_all(&tmp);
        return;
    }
    let _ = Command::new("git")
        .args(["config", "user.email", "t@t.com"])
        .current_dir(&tmp)
        .status();
    let _ = Command::new("git")
        .args(["config", "user.name", "t"])
        .current_dir(&tmp)
        .status();
    std::fs::write(tmp.join("README"), "x\n").unwrap();
    let _ = Command::new("git")
        .args(["add", "README"])
        .current_dir(&tmp)
        .status();
    let _ = Command::new("git")
        .args(["commit", "-m", "i"])
        .current_dir(&tmp)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let out = Command::new(bin())
        .args(["diff"])
        .current_dir(&tmp)
        .output()
        .expect("run next-hunk");
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(
        !out.status.success(),
        "illegal layout must fail startup, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("layout") && (stderr.contains("sidebyside") || stderr.contains("unified")),
        "error must name field + allowed values, got: {stderr}"
    );
}

#[test]
fn focus_on_non_tty_exits_nonzero_with_focus_hint() {
    // Non-TTY + --focus must exit non-zero with a clear message (never silent).
    // Path resolution vs "requires tty" both qualify — the miss is not silent.
    let patch = fixture("tiny_simple.patch");
    let mut child = Command::new(bin())
        .args(["patch", "-", "--focus", "does-not-exist.rs"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(patch.as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("focus")
            && (stderr.contains("not found")
                || stderr.contains("interactive")
                || stderr.contains("tty")),
        "non-tty focus miss must not be silent, got: {stderr}"
    );
}

#[test]
fn inspect_json_emits_review_shape() {
    let out = Command::new(bin())
        .args(["inspect", "--json", "fixtures/tiny_simple.patch"])
        .output()
        .expect("run next-hunk");
    assert!(
        out.status.success(),
        "inspect --json should succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Shape aligned with `next-hunk review` (no serve required).
    for key in [
        "\"file_count\"",
        "\"stream_len\"",
        "\"inserts\"",
        "\"deletes\"",
        "\"files\"",
        "\"display_path\"",
        "\"hunks\"",
    ] {
        assert!(
            stdout.contains(key),
            "inspect --json missing {key}: {stdout}"
        );
    }
    assert!(
        stdout.contains("src/a.rs") && stdout.contains("src/b.rs"),
        "expected both fixture files: {stdout}"
    );
    // Must be JSON, not the human text summary.
    assert!(
        !stdout.starts_with("files="),
        "should not emit text inspect summary: {stdout}"
    );
}

#[test]
fn inspect_json_empty_is_zeroed_object() {
    let mut child = Command::new(bin())
        .args(["inspect", "--json", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(b"   \n").unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"file_count\": 0") || stdout.contains("\"file_count\":0"),
        "empty json should report file_count 0: {stdout}"
    );
    assert!(
        stdout.contains("\"files\": []") || stdout.contains("\"files\":[]"),
        "empty json should have empty files: {stdout}"
    );
}

// ─── auto-forward (serve live → diff --focus becomes push) ───────────────────

/// Tiny git repo with one worktree change, for auto-forward CLI tests.
#[cfg(all(unix, feature = "serve"))]
fn make_git_repo_with_change() -> Option<std::path::PathBuf> {
    let git_ok = Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !git_ok {
        eprintln!("warning: git binary not found; skipping auto-forward test");
        return None;
    }
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "nh-autofwd-{}-{}-{}",
        std::process::id(),
        n,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(&dir)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@next-hunk"]);
    git(&["config", "user.name", "Test"]);
    git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("src.rs"), "fn a() {}\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
    std::fs::write(dir.join("src.rs"), "fn a() {}\nfn b() {}\n").unwrap();
    Some(dir)
}

#[cfg(all(unix, feature = "serve"))]
#[test]
fn diff_focus_forwards_to_live_serve_without_tty() {
    // WXB-9: agent runs `diff --focus` headless while human has serve open →
    // auto-forward as push (no second TUI, no TTY required).
    let Some(dir) = make_git_repo_with_change() else {
        return;
    };
    let _guard = DirGuard(dir.clone());
    let repo = next_hunk::source::find_repo(&dir).expect("find_repo");
    let socket = next_hunk::cli_parse::runtime_socket_path(&repo);

    let listener = next_hunk::tui::server::ServerListener::spawn(socket.clone())
        .expect("spawn mock serve listener");
    let got_push = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let got_push_t = got_push.clone();
    let drainer = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let reqs = listener.drain();
            for r in reqs {
                if matches!(
                    r.command,
                    next_hunk::tui::server::ServerCommand::Push { .. }
                ) {
                    got_push_t.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                let _ = r.reply.send(next_hunk::tui::server::ServerReply::Ok);
            }
            if got_push_t.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            if std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    });

    let out = Command::new(bin())
        .args([
            "diff",
            "--focus",
            "src.rs",
            "--note",
            "banner=please review",
        ])
        .current_dir(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run next-hunk diff");

    assert!(
        out.status.success(),
        "diff --focus with live serve should succeed headless: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("forwarded") || stdout.contains("pushed"),
        "expected forward confirmation, got: {stdout}"
    );
    drainer.join().unwrap();
    assert!(
        got_push.load(std::sync::atomic::Ordering::SeqCst),
        "mock serve should have received a Push command"
    );
}

#[cfg(all(unix, feature = "serve"))]
#[test]
fn diff_focus_no_forward_keeps_non_tty_error() {
    // --no-forward disables auto-forward; non-TTY + --focus must still error
    // (same as when no serve is running).
    let Some(dir) = make_git_repo_with_change() else {
        return;
    };
    let _guard = DirGuard(dir.clone());
    let repo = next_hunk::source::find_repo(&dir).expect("find_repo");
    let socket = next_hunk::cli_parse::runtime_socket_path(&repo);
    // Live serve present, but --no-forward must not use it.
    let listener = next_hunk::tui::server::ServerListener::spawn(socket).expect("spawn mock serve");
    let drainer = std::thread::spawn(move || {
        // Drain so a mistaken push does not hang the client.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            for r in listener.drain() {
                let _ = r.reply.send(next_hunk::tui::server::ServerReply::Ok);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    });

    let out = Command::new(bin())
        .args(["diff", "--no-forward", "--focus", "src.rs"])
        .current_dir(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run next-hunk diff");

    assert!(
        !out.status.success(),
        "--no-forward + non-tty --focus should exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("interactive") || stderr.contains("tty"),
        "expected TTY error, got: {stderr}"
    );
    let _ = drainer.join();
}

#[cfg(all(unix, feature = "serve"))]
#[test]
fn diff_focus_without_serve_errors_non_tty() {
    // No live serve → same headless failure as before (one-shot needs TTY).
    let Some(dir) = make_git_repo_with_change() else {
        return;
    };
    let _guard = DirGuard(dir.clone());
    let out = Command::new(bin())
        .args(["diff", "--focus", "src.rs"])
        .current_dir(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run next-hunk diff");
    assert!(
        !out.status.success(),
        "no serve + non-tty --focus should exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("interactive") || stderr.contains("tty"),
        "expected TTY error, got: {stderr}"
    );
}

#[cfg(all(unix, feature = "serve"))]
struct DirGuard(std::path::PathBuf);
#[cfg(all(unix, feature = "serve"))]
impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// MCP stdio: initialize + tools/list without a live serve (no TTY required).
#[cfg(all(unix, feature = "mcp"))]
#[test]
fn mcp_stdio_initialize_and_list_tools() {
    use std::io::Write;

    let mut child = Command::new(bin())
        .args(["mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn next-hunk mcp");

    {
        let mut stdin = child.stdin.take().expect("stdin");
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2024-11-05","capabilities":{{}},"clientInfo":{{"name":"test","version":"0"}}}}}}"#
        )
        .unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
        )
        .unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{{}}}}"#
        )
        .unwrap();
        // Closing stdin ends the MCP server loop.
    }

    let out = child.wait_with_output().expect("wait mcp");
    assert!(
        out.status.success(),
        "mcp exit non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut saw_init = false;
    let mut saw_tools = false;
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("invalid JSON-RPC line `{line}`: {e}"));
        match v.get("id").and_then(|i| i.as_u64()) {
            Some(1) => {
                assert_eq!(v["result"]["serverInfo"]["name"], "next-hunk");
                assert!(v["result"]["capabilities"]["tools"].is_object());
                saw_init = true;
            }
            Some(2) => {
                let tools = v["result"]["tools"].as_array().expect("tools array");
                assert_eq!(tools.len(), 7);
                let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
                for expected in [
                    "list_sessions",
                    "review_structure",
                    "navigate",
                    "add_comment",
                    "get_decision",
                    "push_focus_note",
                    "reload",
                ] {
                    assert!(names.contains(&expected), "missing tool {expected}");
                }
                saw_tools = true;
            }
            _ => {}
        }
    }
    assert!(saw_init, "missing initialize response in:\n{stdout}");
    assert!(saw_tools, "missing tools/list response in:\n{stdout}");
}

/// MCP tool errors are structured (`isError`) when no serve is live.
#[cfg(all(unix, feature = "mcp"))]
#[test]
fn mcp_get_decision_without_serve_is_tool_error() {
    use std::io::Write;

    let Some(dir) = make_git_repo_with_change() else {
        return;
    };
    let _guard = DirGuard(dir.clone());

    let mut child = Command::new(bin())
        .args(["mcp"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn next-hunk mcp");

    {
        let mut stdin = child.stdin.take().expect("stdin");
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"get_decision","arguments":{{}}}}}}"#
        )
        .unwrap();
    }

    let out = child.wait_with_output().expect("wait mcp");
    assert!(
        out.status.success(),
        "mcp should exit 0 even on tool errors: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut found = false;
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line).expect("json");
        if v.get("id").and_then(|i| i.as_u64()) == Some(1) {
            assert_eq!(v["result"]["isError"], true);
            let text = v["result"]["content"][0]["text"].as_str().unwrap_or("");
            assert!(
                text.contains("error") || text.contains("server") || text.contains("no next-hunk"),
                "unexpected tool error body: {text}"
            );
            found = true;
        }
    }
    assert!(found, "missing tools/call response in:\n{stdout}");
}
