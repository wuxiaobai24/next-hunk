//! Source adapters: produce unified-diff text for the IR layer.
//!
//! Git is accessed exclusively via **gix** (gitoxide). There is no
//! subprocess fallback to the `git` CLI.

mod git;
mod vcs;

pub use git::{
    find_repo, git_diff, git_diff_target, git_file_diff, git_show, open_repo, rev_resolves,
};
pub use vcs::{detect, find_marker_root, jj_diff, jj_show, sl_diff, sl_show, Vcs};
