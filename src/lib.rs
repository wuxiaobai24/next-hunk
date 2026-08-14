//! next-hunk library root.
//!
//! Layers:
//! - [`ir`] — compact runtime diff IR + viewport queries
//! - [`source`] — load unified diffs (gix; no `git` CLI fallback)
//! - [`highlight`] — syntax highlighting (syntect, feature-gated)
//! - [`tui`] — interactive review UI
//! - [`cli_parse`] — parsing for agent-bridge CLI specs (`--focus` / `--note`)
//! - [`cli`] — subcommand parsing/handlers shared by the binaries

pub mod cli;
pub mod cli_parse;
pub mod config;
pub mod highlight;
pub mod ir;
pub mod source;
pub mod tui;
