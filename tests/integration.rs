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

use next_hunk::cli_parse::{runtime_socket_hash, runtime_socket_path};
use next_hunk::config::{DiffRequest, DiffScope};
use next_hunk::ir::FileOrigin;
use next_hunk::ir::{parse_unified_diff, DiffLineKind, Viewport, ViewportQuery};
use next_hunk::source::{
    find_repo, git_diff, git_diff_produced, git_diff_request, git_file_diff, git_show,
    list_repo_worktree_roots, open_repo,
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
    let text = git_diff(&repo.workdir(), DiffScope::Worktree, &[], false).unwrap();
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
    let text = git_diff(&repo.workdir(), DiffScope::Staged, &[], false).unwrap();
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
    let live = git_diff(&repo.workdir(), DiffScope::Worktree, &[], false).unwrap();

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
    let text = git_diff(
        &repo.workdir(),
        DiffScope::Worktree,
        &["src/".to_string()],
        false,
    )
    .unwrap();
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
    let text = git_diff(&repo.workdir(), DiffScope::Worktree, &[], false).unwrap();
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
    let text_without = git_diff(&repo.workdir(), DiffScope::Worktree, &[], false).unwrap();
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
    let text_with = git_diff(&repo.workdir(), DiffScope::Worktree, &[], true).unwrap();
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

/// Dogfood P1: tool-owned `.next-hunk/` must never appear as untracked review
/// content (writing config then `--include-untracked` used to list itself).
#[test]
fn untracked_excludes_next_hunk_config_dir() {
    let Some(_) = require_git() else { return };
    let repo = setup_repo();
    let cfg_dir = repo.path().join(".next-hunk");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    write(&cfg_dir.join("config.toml"), "layout = \"unified\"\n");
    write(&repo.path().join("real_untracked.txt"), "keep me\n");

    let text = git_diff(&repo.workdir(), DiffScope::Worktree, &[], true).unwrap();
    if text.trim().is_empty() {
        panic!("expected at least real_untracked.txt in the diff");
    }
    let review = parse_unified_diff(&text).unwrap();
    let paths: Vec<&str> = review
        .files
        .iter()
        .map(|f| f.display_path.as_str())
        .collect();
    assert!(
        paths.iter().any(|p| p.ends_with("real_untracked.txt")),
        "real untracked file should still appear: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.contains(".next-hunk")),
        ".next-hunk/* must be excluded from untracked review: {paths:?}"
    );
}

/// Dogfood P0: staged + unstaged + untracked must appear together under
/// `scope = working-set` (`--all --include-untracked`), with origin marks.
#[test]
fn working_set_shows_staged_unstaged_and_untracked() {
    let Some(_) = require_git() else { return };
    let repo = setup_repo();
    // setup_repo already has staged (lib.rs, new.rs) and unstaged (README, lib.rs).
    // Add an untracked file for the third bucket.
    write(&repo.path().join("dogfood_untracked.txt"), "untracked\n");

    // Worktree-only must miss staged-only files like src/new.rs (fully staged).
    let wt = git_diff(&repo.workdir(), DiffScope::Worktree, &[], false).unwrap();
    let wt_review = parse_unified_diff(&wt).unwrap();
    let wt_paths: Vec<&str> = wt_review
        .files
        .iter()
        .map(|f| f.display_path.as_str())
        .collect();
    assert!(
        !wt_paths.iter().any(|p| p.ends_with("src/new.rs")),
        "worktree scope should not include fully-staged new.rs: {wt_paths:?}"
    );

    // Staged-only must miss unstaged-only README and untracked.
    let staged = git_diff(&repo.workdir(), DiffScope::Staged, &[], false).unwrap();
    let staged_review = parse_unified_diff(&staged).unwrap();
    let staged_paths: Vec<&str> = staged_review
        .files
        .iter()
        .map(|f| f.display_path.as_str())
        .collect();
    assert!(
        !staged_paths.iter().any(|p| p.ends_with("README.md")),
        "staged scope should not include unstaged README: {staged_paths:?}"
    );

    // Working-set with untracked: all three buckets in one command.
    let produced = git_diff_produced(&repo.workdir(), DiffScope::WorkingSet, &[], true).unwrap();
    assert!(!produced.is_empty());
    let mut review = parse_unified_diff(&produced.text).unwrap();
    review.apply_file_origins(&produced.origins);

    let paths: Vec<&str> = review
        .files
        .iter()
        .map(|f| f.display_path.as_str())
        .collect();
    assert!(
        paths.iter().any(|p| p.ends_with("src/new.rs")),
        "working-set should include staged new.rs: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.ends_with("README.md")),
        "working-set should include unstaged README: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.ends_with("dogfood_untracked.txt")),
        "working-set should include untracked file: {paths:?}"
    );

    // Origins: at least one of each mark when all buckets present.
    let origins: Vec<Option<FileOrigin>> = review.files.iter().map(|f| f.origin).collect();
    assert!(
        origins.contains(&Some(FileOrigin::Staged)),
        "expected a staged origin mark: {origins:?}"
    );
    assert!(
        origins.contains(&Some(FileOrigin::Modified)),
        "expected a modified origin mark: {origins:?}"
    );
    assert!(
        origins.contains(&Some(FileOrigin::Untracked)),
        "expected an untracked origin mark: {origins:?}"
    );

    // Large-diff path still goes through IR + viewport (no full-line materialization required).
    let rows = ViewportQuery::rows(
        &review,
        Viewport {
            start: 0,
            height: review.stream_len.min(20),
        },
        &std::collections::HashSet::new(),
    );
    assert!(!rows.is_empty());
    assert!(rows.len() <= 20);
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

/// Linked worktrees must list as separate roots and bind distinct sockets.
#[test]
fn linked_worktrees_have_independent_session_sockets() {
    let Some(_) = require_git() else { return };
    let repo = setup_repo();
    let main = repo.workdir();

    // Need at least one commit for `git worktree add`.
    // setup_repo already committed; add a second worktree next to the main dir.
    let linked = repo.path().parent().unwrap().join(format!(
        "next-hunk-wt-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    git_in(
        &main,
        &["worktree", "add", "-q", linked.to_str().unwrap(), "HEAD"],
    );
    let _cleanup = LinkedWorktreeGuard {
        main: main.clone(),
        linked: linked.clone(),
    };

    // Discovery from main and from the linked worktree must both see both roots.
    let from_main = list_repo_worktree_roots(&main).unwrap();
    let from_linked = list_repo_worktree_roots(&linked).unwrap();
    assert!(
        from_main.len() >= 2,
        "expected main + linked worktree, got {from_main:?}"
    );
    assert_eq!(
        from_main.len(),
        from_linked.len(),
        "main and linked discovery should agree: {from_main:?} vs {from_linked:?}"
    );

    let main_abs = fs::canonicalize(&main).unwrap_or(main.clone());
    let linked_abs = fs::canonicalize(&linked).unwrap_or(linked.clone());
    assert!(
        from_main
            .iter()
            .any(|p| { fs::canonicalize(p).unwrap_or_else(|_| p.clone()) == main_abs }),
        "main worktree missing from list: {from_main:?}"
    );
    assert!(
        from_main
            .iter()
            .any(|p| { fs::canonicalize(p).unwrap_or_else(|_| p.clone()) == linked_abs }),
        "linked worktree missing from list: {from_main:?}"
    );

    // Socket paths / hashes must differ so two `serve` processes do not collide.
    let sock_main = runtime_socket_path(&main_abs);
    let sock_linked = runtime_socket_path(&linked_abs);
    assert_ne!(
        sock_main, sock_linked,
        "linked worktrees must not share a serve socket"
    );
    assert_ne!(
        runtime_socket_hash(&main_abs),
        runtime_socket_hash(&linked_abs)
    );

    // On Unix, both sockets must be bindable at once (acceptance: no socket steal).
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixListener;
        let _ = fs::remove_file(&sock_main);
        let _ = fs::remove_file(&sock_linked);
        let a = UnixListener::bind(&sock_main)
            .unwrap_or_else(|e| panic!("bind main socket {}: {e}", sock_main.display()));
        let b = UnixListener::bind(&sock_linked)
            .unwrap_or_else(|e| panic!("bind linked socket {}: {e}", sock_linked.display()));
        drop(a);
        drop(b);
        let _ = fs::remove_file(&sock_main);
        let _ = fs::remove_file(&sock_linked);
    }
}

/// Best-effort cleanup for a linked worktree created in tests.
struct LinkedWorktreeGuard {
    main: PathBuf,
    linked: PathBuf,
}

impl Drop for LinkedWorktreeGuard {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&self.linked)
            .current_dir(&self.main)
            .output();
        let _ = fs::remove_dir_all(&self.linked);
    }
}

/// Branch with commits on `main` and `feature`, plus uncommitted worktree edits.
/// Used for `--base` / `--range` / merge-base reviews.
fn setup_branch_repo() -> RepoGuard {
    let repo = RepoGuard::new();
    let root = repo.path();
    write(&root.join("base.txt"), "base-v1\n");
    write(&root.join("shared.txt"), "shared-v1\n");
    git_in(root, &["add", "."]);
    git_in(root, &["commit", "-q", "-m", "main base"]);
    // Ensure the default branch is named main for explicit --base main tests.
    git_in(root, &["branch", "-M", "main"]);

    git_in(root, &["checkout", "-q", "-b", "feature"]);
    write(&root.join("feature.txt"), "feature only\n");
    write(&root.join("shared.txt"), "shared-v2-on-feature\n");
    git_in(root, &["add", "."]);
    git_in(root, &["commit", "-q", "-m", "feature commit"]);

    // Uncommitted worktree edit on the feature branch.
    write(&root.join("shared.txt"), "shared-v3-worktree\n");
    write(&root.join("wip.txt"), "untracked-ish worktree file\n");
    git_in(root, &["add", "wip.txt"]); // staged new file
    write(&root.join("wip.txt"), "staged then further edited\n"); // unstaged on top
    repo
}

#[test]
fn base_diff_reviews_full_branch_plus_worktree() {
    let Some(_) = require_git() else { return };
    let repo = setup_branch_repo();
    let request = DiffRequest::AgainstBase {
        base: "main".into(),
        use_merge_base: false,
    };
    let produced = git_diff_request(&repo.workdir(), &request, &[], false).unwrap();
    assert!(
        !produced.is_empty(),
        "base main..worktree should not be empty"
    );

    let review = parse_unified_diff(&produced.text).unwrap();
    let paths: Vec<&str> = review
        .files
        .iter()
        .map(|f| f.display_path.as_str())
        .collect();
    assert!(
        paths.iter().any(|p| p.ends_with("feature.txt")),
        "branch-added file should appear vs main: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.ends_with("shared.txt")),
        "shared file changed on branch+worktree should appear: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.ends_with("wip.txt")),
        "local staged/worktree file should appear vs main: {paths:?}"
    );

    // Viewport IR path: materialize only a window, not the full widget tree.
    let rows = ViewportQuery::rows(
        &review,
        Viewport {
            start: 0,
            height: review.stream_len.min(16),
        },
        &std::collections::HashSet::new(),
    );
    assert!(!rows.is_empty());
    assert!(rows.len() <= 16);
}

#[test]
fn base_merge_base_strategy_uses_fork_point() {
    let Some(_) = require_git() else { return };
    let repo = setup_branch_repo();
    // Discard uncommitted noise so we can switch branches and advance main.
    git_in(repo.path(), &["reset", "--hard", "-q"]);
    git_in(repo.path(), &["clean", "-fdq"]);
    // Advance main after the fork so direct base vs merge-base can differ in
    // general; here we only assert merge-base mode succeeds and includes the
    // feature commit files.
    git_in(repo.path(), &["checkout", "-q", "main"]);
    write(&repo.path().join("base.txt"), "base-v2-on-main\n");
    git_in(repo.path(), &["add", "base.txt"]);
    git_in(repo.path(), &["commit", "-q", "-m", "main advances"]);
    git_in(repo.path(), &["checkout", "-q", "feature"]);

    let request = DiffRequest::AgainstBase {
        base: "main".into(),
        use_merge_base: true,
    };
    let produced = git_diff_request(&repo.workdir(), &request, &[], false).unwrap();
    let review = parse_unified_diff(&produced.text).unwrap();
    let paths: Vec<&str> = review
        .files
        .iter()
        .map(|f| f.display_path.as_str())
        .collect();
    assert!(
        paths.iter().any(|p| p.ends_with("feature.txt")),
        "merge-base review should still show feature-only files: {paths:?}"
    );
}

#[test]
fn range_diff_matches_show() {
    let Some(_) = require_git() else { return };
    let repo = setup_branch_repo();
    let show = git_show(&repo.workdir(), "main..HEAD").unwrap();
    let produced = git_diff_request(
        &repo.workdir(),
        &DiffRequest::Range("main..HEAD".into()),
        &[],
        false,
    )
    .unwrap();
    assert_eq!(
        produced.text, show,
        "diff --range should match show for the same revspec"
    );
    let review = parse_unified_diff(&produced.text).unwrap();
    assert!(review.file_count() >= 1);
}
