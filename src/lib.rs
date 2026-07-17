//! next-hunk library root.
//!
//! Layers:
//! - [`ir`] — compact runtime diff IR + viewport queries
//! - [`source`] — load unified diffs (gix for git; `jj` CLI for Jujutsu)
//! - [`highlight`] — syntax highlighting (syntect, feature-gated)
//! - [`tui`] — interactive review UI
//! - [`cli_parse`] — parsing for agent-bridge CLI specs (`--focus` / `--note`)
//! - [`session_client`] — shared live-serve client (CLI + MCP; serve+unix)
//! - [`mcp`] — optional MCP stdio control plane (feature `mcp`)

pub mod cli_parse;
pub mod config;
pub mod highlight;
pub mod ir;
pub mod source;
pub mod tui;

#[cfg(all(feature = "serve", unix))]
pub mod session_client;

#[cfg(feature = "mcp")]
pub mod mcp;
