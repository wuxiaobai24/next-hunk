//! next-hunk library root.
//!
//! Layers:
//! - [`ir`] — compact runtime diff IR + viewport queries
//! - [`source`] — load unified diffs (gix; no `git` CLI fallback)
//! - [`tui`] — interactive review UI

pub mod ir;
pub mod source;
pub mod tui;
