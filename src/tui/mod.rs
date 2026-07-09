//! Interactive review TUI (Phase 2).
//!
//! Intentionally incomplete: engine (IR + viewport) lands first.
//! When implemented, this module owns draw/keys/scroll only — never a second
//! full-line cache of the entire review.

use anyhow::{bail, Result};

use crate::ir::Review;

/// Run the interactive review UI over an already-parsed [`Review`].
///
/// Phase 2 will implement ratatui rail + virtualized stream. Until then this
/// returns a clear error so CLI wiring can call it without panicking on
/// missing modules.
pub fn run_review_tui(_review: &Review) -> Result<()> {
    bail!("TUI is not implemented yet (Phase 2); IR/viewport engine is available as a library")
}
