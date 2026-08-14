//! Real-git integration tests.
//!
//! These exercise the full source-adapter → IR → viewport pipeline against a
//! real temporary git repository created with the `git` CLI. The *product*
//! stays gix-only (no `git` CLI at runtime, per architecture §2.3.7); this is
//! a test-only convenience to set up realistic repo state.
//!
//! If the `git` binary is not present, these tests skip (not fail) so CI
//! environments without git don't go red.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use next_hunk::ir::{parse_unified_diff, DiffLineKind, Viewport, ViewportQuery};
use next_hunk::source::{
    find_repo, git_diff, git_diff_target, git_file_diff, git_show, open_repo, rev_resolves,
};

/// Skip the test if `git` is unavailable.
fn require_git() -> Option<()> {
    let ok = Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        Some(())
    } else {
        eprintln!("warning: git binary not found; skipping git integration test");
        None
    }
}

/// A temporary git repo. Cleaned up on drop.
struct RepoGuard {
    dir: PathBuf,
}

impl RepoGuard {
    fn new() -> RepoGuard {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let uniq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "next-hunk-it-{}-{}-{}",
            std::process::id(),
            uniq,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        // isolate from the host's git identity
        git_in(&dir, &["init", "-q"]);
        git_in(&dir, &["config", "user.email", "test@next-hunk"]);
        git_in(&dir, &["config", "user.name", "Test"]);
        git_in(&dir, &["config", "commit.gpgsign", "false"]);
        RepoGuard { dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn workdir(&self) -> PathBuf {
        // find_repo returns the worktree root
        find_repo(&self.dir).unwrap()
    }
}

impl Drop for RepoGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git {:?} in {}: {e}", args, dir.display()));
    if !out.status.success() {
        panic!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

/// Set up a repo with one committed file, then make both staged and worktree
/// changes so all diff modes have something to show.
fn setup_repo() -> RepoGuard {
    let repo = RepoGuard::new();
    let root = repo.path();
    // initial commit
    write(&root.join("src/lib.rs"), "fn a() {}\nfn b() {}\n");
    write(&root.join("README.md"), "hello\n");
    git_in(root, &["add", "."]);
    git_in(root, &["commit", "-q", "-m", "initial"]);

    // staged change: modify lib.rs and add a new file
    write(
        &root.join("src/lib.rs"),
        "fn a() {}\nfn b() {}\nfn c() {}\n",
    );
    write(&root.join("src/new.rs"), "pub fn new() {}\n");
    git_in(root, &["add", "."]);

    // worktree (unstaged) changes: modify README AND src/lib.rs further so a
    // worktree diff under src/ has real content (untracked files are excluded
    // by the adapter).
    write(&root.join("README.md"), "hello world\nchanged\n");
    write(
        &root.join("src/lib.rs"),
        "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\n",
    );
    repo
}

#[test]
fn worktree_diff_round_trips() {
    let Some(_) = require_git() else { return };
    let repo = setup_repo();
    let text = git_diff(&repo.workdir(), false, &[], false).unwrap();
    assert!(!text.trim().is_empty(), "worktree diff should not be empty");

    let review = parse_unified_diff(&text).unwrap();
    assert!(review.file_count() >= 1);

    // README should appear as a modified file in the worktree diff
    let paths: Vec<&str> = review
        .files
        .iter()
        .map(|f| f.display_path.as_str())
        .collect();
    assert!(
        paths.iter().any(|p| p.ends_with("README.md")),
        "README.md should be in worktree diff: {paths:?}"
    );

    // the changed README content should surface as add lines
    let readme = review
        .files
        .iter()
        .find(|f| f.display_path.ends_with("README.md"))
        .unwrap();
    let has_add = readme
        .hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .any(|l| l.kind == DiffLineKind::Add);
    assert!(has_add, "README should have added lines");
}

#[test]
fn staged_diff_round_trips() {
    let Some(_) = require_git() else { return };
    let repo = setup_repo();
    let text = git_diff(&repo.workdir(), true, &[], false).unwrap();
    assert!(!text.trim().is_empty(), "staged diff should not be empty");

    let review = parse_unified_diff(&text).unwrap();
    // staged changes include src/lib.rs modification and src/new.rs addition
    let paths: Vec<&str> = review
        .files
        .iter()
        .map(|f| f.display_path.as_str())
        .collect();
    assert!(
        paths.iter().any(|p| p.ends_with("lib.rs")),
        "staged diff should include lib.rs: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.ends_with("new.rs")),
        "staged diff should include new.rs: {paths:?}"
    );

    // README is NOT staged (only worktree-modified), so it must be absent
    assert!(
        !paths.iter().any(|p| p.ends_with("README.md")),
        "README should not be in staged diff: {paths:?}"
    );
}

#[test]
fn show_head_matches_commit() {
    let Some(_) = require_git() else { return };
    let repo = setup_repo();
    let text = git_show(&repo.workdir(), "HEAD").unwrap();
    assert!(!text.trim().is_empty());

    let review = parse_unified_diff(&text).unwrap();
    // HEAD commit added src/lib.rs and README.md initially
    assert!(review.file_count() >= 1);
    let paths: Vec<&str> = review
        .files
        .iter()
        .map(|f| f.display_path.as_str())
        .collect();
    assert!(
        paths.iter().any(|p| p.ends_with("lib.rs")),
        "HEAD show should include lib.rs: {paths:?}"
    );
}

#[test]
fn patch_stdin_round_trips_identically_to_live_diff() {
    let Some(_) = require_git() else { return };
    let repo = setup_repo();
    let live = git_diff(&repo.workdir(), false, &[], false).unwrap();

    // Simulate the `patch -` CLI path: take the live diff text, parse it back.
    let review_live = parse_unified_diff(&live).unwrap();

    // Write the diff to a temp patch file and re-read (mirrors read_patch_input)
    let patch_path = repo.path().join("snapshot.patch");
    fs::write(&patch_path, &live).unwrap();
    let reread = fs::read_to_string(&patch_path).unwrap();
    let review_patch = parse_unified_diff(&reread).unwrap();

    // The two reviews should be structurally identical
    assert_eq!(review_live.file_count(), review_patch.file_count());
    assert_eq!(review_live.stream_len, review_patch.stream_len);
    for (a, b) in review_live.files.iter().zip(review_patch.files.iter()) {
        assert_eq!(a.display_path, b.display_path);
        assert_eq!(a.hunks.len(), b.hunks.len());
    }
}

#[test]
fn pathspec_filters_files() {
    let Some(_) = require_git() else { return };
    let repo = setup_repo();
    // Only ask for src/ — should exclude README.md from worktree diff
    let text = git_diff(&repo.workdir(), false, &["src/".to_string()], false).unwrap();
    let review = parse_unified_diff(&text).unwrap();

    let paths: Vec<&str> = review
        .files
        .iter()
        .map(|f| f.display_path.as_str())
        .collect();
    assert!(
        !paths.iter().any(|p| p.ends_with("README.md")),
        "pathspec src/ should exclude README.md: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.starts_with("src/")),
        "pathspec src/ should include a src/ file: {paths:?}"
    );
}

#[test]
fn viewport_materializes_live_diff() {
    let Some(_) = require_git() else { return };
    let repo = setup_repo();
    let text = git_diff(&repo.workdir(), false, &[], false).unwrap();
    let review = parse_unified_diff(&text).unwrap();

    // materialize the whole stream and verify row types are sane
    let rows = ViewportQuery::rows(
        &review,
        Viewport {
            start: 0,
            height: review.stream_len,
        },
        &std::collections::HashSet::new(),
    );
    assert_eq!(rows.len(), review.stream_len);
    assert!(matches!(
        rows.first().unwrap(),
        next_hunk::ir::StreamRow::FileHeader { .. }
    ));

    // a viewport starting mid-stream still maps back to a valid file
    if review.stream_len > 2 {
        let mid = review.stream_len / 2;
        let file_idx = ViewportQuery::file_at_row(&review, mid);
        assert!(file_idx.is_some());
    }
}

#[test]
fn worktree_diff_untracked_included_when_enabled() {
    let Some(_) = require_git() else { return };
    let repo = setup_repo();
    // Create an untracked file
    write(&repo.path().join("untracked.txt"), "untracked content\n");
    // Without --include-untracked, untracked file should NOT appear
    let text_without = git_diff(&repo.workdir(), false, &[], false).unwrap();
    let review_without = parse_unified_diff(&text_without).unwrap();
    let paths_without: Vec<&str> = review_without
        .files
        .iter()
        .map(|f| f.display_path.as_str())
        .collect();
    assert!(
        !paths_without.iter().any(|p| p.ends_with("untracked.txt")),
        "untracked file should NOT appear without include_untracked: {paths_without:?}"
    );
    // With --include-untracked, untracked file SHOULD appear
    let text_with = git_diff(&repo.workdir(), false, &[], true).unwrap();
    let review_with = parse_unified_diff(&text_with).unwrap();
    let paths_with: Vec<&str> = review_with
        .files
        .iter()
        .map(|f| f.display_path.as_str())
        .collect();
    assert!(
        paths_with.iter().any(|p| p.ends_with("untracked.txt")),
        "untracked file should appear with include_untracked=true: {paths_with:?}"
    );
    // Verify it's rendered as a new file addition
    let untracked = review_with
        .files
        .iter()
        .find(|f| f.display_path.ends_with("untracked.txt"))
        .unwrap();
    let has_add = untracked
        .hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .any(|l| l.kind == DiffLineKind::Add);
    assert!(has_add, "untracked file should have added lines");
}

#[test]
fn filediff_two_files_produces_unified_diff() {
    let Some(_) = require_git() else { return };
    let repo = setup_repo();
    let repo_handle = open_repo(&repo.workdir()).unwrap();

    // Create two files to diff
    let old_path = repo.path().join("old_file.txt");
    let new_path = repo.path().join("new_file.txt");
    write(&old_path, "line1\nline2\nline3\n");
    write(&new_path, "line1\nmodified\nline3\n");

    let text = git_file_diff(&repo_handle, &old_path, &new_path).unwrap();
    assert!(!text.trim().is_empty(), "file diff should not be empty");

    let review = parse_unified_diff(&text).unwrap();
    assert_eq!(review.file_count(), 1, "should have exactly one file");

    let file = &review.files[0];
    // Should contain a diff header with both paths
    assert!(
        file.display_path.contains("old_file.txt") || file.display_path.contains("new_file.txt"),
        "file path should appear in display: {}",
        file.display_path
    );

    // Should contain modified lines (add + delete for the changed line)
    let has_add = file
        .hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .any(|l| l.kind == DiffLineKind::Add);
    let has_del = file
        .hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .any(|l| l.kind == DiffLineKind::Delete);
    assert!(has_add, "should have added lines");
    assert!(has_del, "should have deleted lines");

    // Identical files produce no hunk content
    let same_text = git_file_diff(&repo_handle, &old_path, &old_path).unwrap();
    // May have a diff header but no @@ hunk lines
    assert!(
        !same_text.contains("@@"),
        "identical files should produce no hunk lines, got: {same_text}"
    );
}

#[test]
fn filediff_with_relative_paths() {
    let Some(_) = require_git() else { return };
    let repo = setup_repo();
    let repo_handle = open_repo(&repo.workdir()).unwrap();

    // Create files inside the repo, use relative paths
    write(&repo.path().join("a.txt"), "hello\n");
    write(&repo.path().join("b.txt"), "hello\nworld\n");

    let text = git_file_diff(&repo_handle, Path::new("a.txt"), Path::new("b.txt")).unwrap();
    assert!(!text.trim().is_empty());
    let review = parse_unified_diff(&text).unwrap();
    assert_eq!(review.file_count(), 1);
}

// ─── diff <target> ───────────────────────────────────────────────────────────

/// Repo state that exercises every tree-vs-worktree case: modified (staged +
/// unstaged), added-since-target, deleted-from-disk.
fn setup_target_repo() -> RepoGuard {
    let repo = RepoGuard::new();
    let root = repo.path();
    write(&root.join("a.txt"), "line1\nline2\n");
    write(&root.join("b.txt"), "to be deleted\n");
    git_in(root, &["add", "."]);
    git_in(root, &["commit", "-q", "-m", "base"]);

    // staged modification of a.txt, staged addition of c.txt …
    write(&root.join("a.txt"), "line1\nline2 staged\n");
    write(&root.join("c.txt"), "added after base\n");
    git_in(root, &["add", "."]);
    // … and a further unstaged edit of a.txt plus a deletion of b.txt on disk.
    write(&root.join("a.txt"), "line1\nline2 staged\nline3 unstaged\n");
    fs::remove_file(root.join("b.txt")).unwrap();
    repo
}

/// The file set of `nh diff HEAD` (tree vs worktree) matches `git diff HEAD`.
#[test]
fn diff_target_head_matches_git() {
    let Some(_) = require_git() else { return };
    let repo = setup_target_repo();
    let workdir = repo.workdir();

    let text = git_diff_target(&workdir, "HEAD", &[], false).unwrap();
    let review = parse_unified_diff(&text).unwrap();
    let mut paths: Vec<String> = review
        .files
        .iter()
        .map(|f| f.display_path.clone())
        .collect();
    paths.sort();

    let git_status = git_in(&workdir, &["diff", "HEAD", "--name-only"]);
    let mut git_paths: Vec<String> = git_status.lines().map(str::to_string).collect();
    git_paths.sort();

    assert_eq!(
        paths, git_paths,
        "diff HEAD file set should match `git diff HEAD`"
    );
    assert_eq!(paths, vec!["a.txt", "b.txt", "c.txt"]);
}

/// A two-rev range goes through the tree-to-tree path (like `git diff A..B`).
#[test]
fn diff_target_range_matches_git() {
    let Some(_) = require_git() else { return };
    let repo = RepoGuard::new();
    let root = repo.path();
    write(&root.join("f.txt"), "v1\n");
    git_in(root, &["add", "."]);
    git_in(root, &["commit", "-q", "-m", "base"]);
    write(&root.join("f.txt"), "v2\n");
    git_in(root, &["add", "."]);
    git_in(root, &["commit", "-q", "-m", "second"]);

    let workdir = repo.workdir();
    let text = git_diff_target(&workdir, "HEAD~1..HEAD", &[], false).unwrap();
    let review = parse_unified_diff(&text).unwrap();
    let paths: Vec<&str> = review
        .files
        .iter()
        .map(|f| f.display_path.as_str())
        .collect();
    assert_eq!(paths, vec!["f.txt"]);
    assert_eq!(review.inserts, 1);
    assert_eq!(review.deletes, 1);
}

/// `--staged` with a target diffs that tree against the index
/// (like `git diff --cached <rev>`).
#[test]
fn diff_target_staged_ignores_worktree_only_changes() {
    let Some(_) = require_git() else { return };
    let repo = setup_target_repo();
    let workdir = repo.workdir();

    let text = git_diff_target(&workdir, "HEAD", &[], true).unwrap();
    let review = parse_unified_diff(&text).unwrap();
    let paths: Vec<&str> = review
        .files
        .iter()
        .map(|f| f.display_path.as_str())
        .collect();
    // b.txt is only deleted from disk (still in the index), so a staged diff
    // against HEAD must not list it; the worktree-only line3 edit in a.txt is
    // equally invisible.
    assert_eq!(paths, vec!["a.txt", "c.txt"]);
}

/// The rev-probe used by the CLI to disambiguate target vs pathspec.
#[test]
fn rev_resolves_probe_variants() {
    let Some(_) = require_git() else { return };
    let repo = setup_target_repo();
    let workdir = repo.workdir();
    assert!(rev_resolves(&workdir, "HEAD"));
    assert!(!rev_resolves(&workdir, "definitely-not-a-rev"));
    assert!(!rev_resolves(&workdir, "a.txt"));
}
