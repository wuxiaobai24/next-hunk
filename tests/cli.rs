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
        stderr.contains("parse") || stderr.contains("empty diff") || stderr.contains("error"),
        "pager should report parse/fatal on stderr, got: {stderr}"
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
