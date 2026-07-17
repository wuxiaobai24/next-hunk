//! Source adapters: produce unified-diff text for the IR layer.
//!
//! Git is accessed exclusively via **gix** (gitoxide). There is no
//! subprocess fallback to the `git` CLI.

mod git;

pub use git::{
    find_repo, git_diff, git_diff_produced, git_file_diff, git_show, list_repo_worktree_roots,
    open_repo, ProducedDiff,
};
