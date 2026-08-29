//! Compact **runtime** diff intermediate representation.
//!
//! "Compact" means the in-memory model for large reviews — not the release
//! binary size. Design goals:
//! - Cheap to build from unified diff text
//! - O(visible) materialization for the TUI (no per-line widget tree for the whole stream)
//! - Stable indices for future highlight / search layers

pub mod collapse;
mod model;
mod parse;
pub mod viewport;
pub mod whitespace;
pub mod worddiff;

pub use collapse::CollapseIndex;
pub use model::{DiffLine, DiffLineKind, FileDiff, Hunk, Review};
pub use parse::{parse_unified_diff, ParseError};
pub use viewport::{PairSide, StreamRow, VRow, Viewport, ViewportQuery};
pub use whitespace::strip_whitespace_changes;
pub use worddiff::{counterpart_text, line_pair_diff, word_diff_regions, WordOp, WordRegion};
