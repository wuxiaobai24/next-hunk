//! next-hunk library root.
//!
//! Layers:
//! - [`ir`] — compact runtime diff IR + viewport queries
//! - [`source`] — load unified diffs (git CLI, later patch/stdin, optional libs)
//! - [`tui`] — interactive review UI (Phase 2)

pub mod ir;
pub mod source;
pub mod tui;
