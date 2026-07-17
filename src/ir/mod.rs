//! Compact **runtime** diff intermediate representation.
//!
//! "Compact" means the in-memory model for large reviews — not the release
//! binary size. Design goals:
//! - Cheap to build from unified diff text
//! - O(visible) materialization for the TUI (no per-line widget tree for the whole stream)
//! - Stable indices for future highlight / search layers

mod incremental;
mod model;
mod parse;
mod summary;
pub mod viewport;
pub mod whitespace;
pub mod worddiff;

pub use incremental::{
    fingerprint_section, parse_unified_diff_full, parse_unified_diff_incremental,
    split_file_sections, FileSection, IncrementalError, IncrementalParseResult, IncrementalStats,
    ReloadMode,
};
pub use model::{DiffLine, DiffLineKind, FileDiff, FileOrigin, Hunk, Review};
pub use parse::{parse_unified_diff, ParseError};
pub use summary::{FileSummary, HunkSummary, ReviewSummary};
pub use viewport::{StreamRow, Viewport, ViewportQuery};
pub use whitespace::strip_whitespace_changes;
pub use worddiff::{counterpart_text, line_pair_diff, word_diff_regions, WordOp, WordRegion};
