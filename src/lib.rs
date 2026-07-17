//! next-hunk library root.
//!
//! Layers:
//! - [`ir`] — compact runtime diff IR + viewport queries
//! - [`source`] — load unified diffs (gix for git; `jj` CLI for Jujutsu)
//! - [`highlight`] — syntax highlighting (syntect, feature-gated)
//! - [`tui`] — interactive review UI
//! - [`cli_parse`] — parsing for agent-bridge CLI specs (`--focus` / `--note`)

pub mod cli_parse;
pub mod config;
pub mod highlight;
pub mod ir;
/// Optional MCP stdio control plane (`--features mcp`).
#[cfg(all(feature = "mcp", feature = "serve", unix))]
pub mod mcp;
/// Shared live-session client (CLI + MCP). Available when `serve` is on (Unix).
#[cfg(all(feature = "serve", unix))]
pub mod session;
pub mod source;
pub mod tui;
