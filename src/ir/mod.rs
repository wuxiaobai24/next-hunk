//! Compact **runtime** diff intermediate representation.
//!
//! "Compact" means the in-memory model for large reviews — not the release
//! binary size. Design goals:
//! - Cheap to build from unified diff text
//! - O(visible) materialization for the TUI (no per-line widget tree for the whole stream)
//! - Stable indices for future highlight / search / export layers

mod model;
mod parse;
mod viewport;

pub use model::{DiffLine, DiffLineKind, FileDiff, Hunk, Review};
pub use parse::{parse_unified_diff, ParseError};
pub use viewport::{StreamRow, Viewport, ViewportQuery};
