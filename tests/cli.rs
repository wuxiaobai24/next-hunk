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

// ─── nh short binary & diff <target> disambiguation ─────────────────────────

/// Skip when the `git` CLI is unavailable (repo setup convenience only; the
/// product itself stays gix-only).
fn require_git() -> Option<()> {
    let ok = Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        Some(())
    } else {
        eprintln!("warning: git binary not found; skipping git CLI test");
        None
    }
}

/// A temp git repo with one commit of src/a.rs and a later modification, for
/// exercising `diff <target>` from the real binary. Cleaned up on drop.
struct TempRepo {
    dir: PathBuf,
}

impl TempRepo {
    fn new() -> TempRepo {
        let dir = std::env::temp_dir().join(format!(
            "nh-cli-{}-{}",
            std::process::id(),
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
                .expect("run git");
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        // `--template=` skips template copying: on the macOS CI runner the
        // homebrew git's template copy has a flaky "File exists" failure when
        // two test repos init concurrently, and the templates are unused here.
        git(&["init", "-q", "--template="]);
        git(&["config", "user.email", "test@next-hunk"]);
        git(&["config", "user.name", "Test"]);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(dir.join("other.txt"), "x\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        // Two worktree modifications so pathspec filtering is observable.
        std::fs::write(dir.join("src").join("a.rs"), "fn a() { changed }\n").unwrap();
        std::fs::write(dir.join("other.txt"), "x changed\n").unwrap();
        TempRepo { dir }
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn nh_binary_matches_next_hunk() {
    // The short alias is the same program: same version, and it accepts the
    // same subcommands (help header/usage should say `nh`).
    let nh = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_nh")))
        .arg("--version")
        .output()
        .expect("run nh");
    assert!(nh.status.success(), "nh --version should succeed");
    let full = Command::new(bin())
        .arg("--version")
        .output()
        .expect("run next-hunk");
    // The version *number* must match (the leading name naturally differs).
    let version_of = |s: String| s.split_whitespace().nth(1).unwrap_or("").to_string();
    assert_eq!(
        version_of(String::from_utf8_lossy(&nh.stdout).into_owned()),
        version_of(String::from_utf8_lossy(&full.stdout).into_owned()),
        "nh and next-hunk should report the same version"
    );

    let help = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_nh")))
        .arg("--help")
        .output()
        .expect("run nh --help");
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(
        help_text.contains("Usage: nh"),
        "nh usage should be branded nh: {help_text}"
    );
}

#[test]
fn diff_target_bad_rev_errors_git_style() {
    let Some(_) = require_git() else { return };
    let repo = TempRepo::new();
    let out = Command::new(bin())
        .args(["diff", "definitely-not-a-rev"])
        .current_dir(&repo.dir)
        .output()
        .expect("run next-hunk diff");
    assert!(!out.status.success(), "diff <bad-rev> should exit non-zero");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown revision or path not in the working tree"),
        "git-style error expected, got: {stderr}"
    );
}

#[test]
fn diff_target_rev_renders_and_path_falls_back() {
    let Some(_) = require_git() else { return };
    let repo = TempRepo::new();

    // `diff HEAD` (piped → inspect fallback) shows both modified files.
    let out = Command::new(bin())
        .args(["diff", "HEAD"])
        .current_dir(&repo.dir)
        .output()
        .expect("run next-hunk diff HEAD");
    assert!(out.status.success(), "diff HEAD should succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("src/a.rs") && stdout.contains("other.txt"),
        "diff HEAD should list both modified files: {stdout}"
    );

    // `diff HEAD -- src` — pathspec after the target filters to src/a.rs.
    let out = Command::new(bin())
        .args(["diff", "HEAD", "--", "src"])
        .current_dir(&repo.dir)
        .output()
        .expect("run next-hunk diff HEAD -- src");
    assert!(out.status.success(), "pathspec filtering should succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("src/a.rs") && !stdout.contains("other.txt"),
        "pathspec should filter to src only: {stdout}"
    );

    // `diff src` — `src` is not a rev but exists on disk, so it falls back to
    // a pathspec (note on stderr) and filters the worktree diff to src/a.rs.
    let out = Command::new(bin())
        .args(["diff", "src"])
        .current_dir(&repo.dir)
        .output()
        .expect("run next-hunk diff src");
    assert!(out.status.success(), "disk-path fallback should succeed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stderr.contains("using it as a pathspec"),
        "fallback should explain itself, stderr: {stderr}"
    );
    assert!(
        stdout.contains("src/a.rs") && !stdout.contains("other.txt"),
        "fallback pathspec should filter to src only: {stdout}"
    );
}
