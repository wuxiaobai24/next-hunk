//! Thin input wrapper over crossterm, kept separate so [`super::app::App`]
//! stays free of terminal I/O and is testable headlessly.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{poll, read, Event};

/// Poll for up to `timeout_ms` for an event. Returns `Ok(None)` on timeout.
pub fn read_event(timeout_ms: u64) -> Result<Option<Event>> {
    if poll(Duration::from_millis(timeout_ms))? {
        Ok(Some(read()?))
    } else {
        Ok(None)
    }
}
