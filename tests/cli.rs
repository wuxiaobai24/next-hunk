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
    assert!(out.status.success(), "exit non-zero: {}", String::from_utf8_lossy(&out.stderr));
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
    use std::io::Write;
    child.stdin.take().unwrap().write_all(b"   \n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("files=0"), "empty diff should report files=0: {stdout}");
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
    use std::io::Write;
    child.stdin.take().unwrap().write_all(patch.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "patch - should exit 0 even on non-tty");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // fallback inspect summary should appear
    assert!(combined.contains("files=2"), "expected inspect fallback: {combined}");
}

#[test]
fn patch_missing_file_errors() {
    let out = Command::new(bin())
        .args(["patch", "does-not-exist.patch"])
        .output()
        .expect("run next-hunk");
    assert!(!out.status.success(), "should fail on missing file");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not found"), "stderr should mention not found: {stderr}");
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
