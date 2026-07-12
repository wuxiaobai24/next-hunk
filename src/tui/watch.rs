//! Filesystem watcher for `--watch` mode.
//!
//! Two compilation modes (mirroring [`crate::highlight`]):
//! - **feature `watch`**: uses `notify` 6 to watch the worktree recursively.
//!   notify fires callbacks on its own thread; we forward them to an mpsc
//!   channel that the TUI main loop drains non-blockingly (architecture §2.3:
//!   the input path stays synchronous and short — no async runtime).
//! - **feature off**: a no-op `Watcher` whose `spawn` always errors, so the
//!   `--watch` CLI flag reports that the feature is missing instead of panicking.
//!
//! Debouncing is handled by the caller (the main loop): events accumulate and a
//! reload only fires after a short quiet period, so a single save that produces
//! many fs events coalesces into one re-parse.

use std::path::Path;

use anyhow::Result;

#[cfg(feature = "watch")]
mod imp {
    use super::*;
    use notify::{RecursiveMode, Watcher as _};
    use std::sync::mpsc;
    use std::time::Duration;

    /// Quiet period after the last fs event before a reload fires. Coalesces
    /// the burst of events a single save typically produces.
    pub const DEBOUNCE: Duration = Duration::from_millis(200);

    /// Holds the notify watcher (kept alive for its lifetime) and the receiving
    /// end of the channel it pushes events into.
    pub struct Watcher {
        rx: mpsc::Receiver<()>,
        // Boxed so the struct stays movable regardless of RecommendedWatcher's
        // generic-inlined size; `_inner` must outlive the channel sender.
        _inner: notify::RecommendedWatcher,
    }

    impl Watcher {
        /// Watch `workdir` recursively for modifications.
        ///
        /// Returns a `Watcher` whose [`Self::drain`] the caller polls each frame.
        pub fn spawn(workdir: &Path) -> Result<Self> {
            let (tx, rx) = mpsc::channel::<()>();
            // Closure-based event handler: forward any event (errors included)
            // as a unit signal. We don't need the event payload — only that
            // *something* changed.
            let handler = move |res: notify::Result<notify::Event>| {
                if res.is_ok() {
                    // best-effort: ignore send errors (receiver dropped on quit)
                    let _ = tx.send(());
                }
            };
            let mut inner = notify::recommended_watcher(handler)
                .map_err(|e| anyhow::anyhow!("init filesystem watcher: {e}"))?;
            inner
                .watch(workdir, RecursiveMode::Recursive)
                .map_err(|e| anyhow::anyhow!("watch {workdir:?}: {e}"))?;
            Ok(Self { rx, _inner: inner })
        }

        /// Non-blocking: drain all pending events. Returns `true` if at least
        /// one event was observed (the caller then applies the debounce delay).
        pub fn drain(&self) -> bool {
            let mut saw = false;
            while self.rx.try_recv().is_ok() {
                saw = true;
            }
            saw
        }

        /// Whether real watching is compiled in.
        pub fn is_enabled() -> bool {
            true
        }
    }
}

#[cfg(not(feature = "watch"))]
mod imp {
    use super::*;

    /// No-op watcher used when the `watch` feature is off.
    pub struct Watcher;

    impl Watcher {
        pub fn spawn(_workdir: &Path) -> Result<Self> {
            anyhow::bail!("built without the `watch` feature; rebuild with `--features watch`")
        }

        pub fn drain(&self) -> bool {
            false
        }

        pub fn is_enabled() -> bool {
            false
        }
    }
}

pub use imp::Watcher;

#[cfg(feature = "watch")]
pub use imp::DEBOUNCE;
#[cfg(not(feature = "watch"))]
/// Placeholder debounce constant for the no-op build (unused).
pub const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(200);

#[cfg(all(test, feature = "watch"))]
mod tests {
    use super::*;
    use std::fs;
    use std::thread;
    use std::time::{Duration, Instant};

    /// Wait up to ~2s for `cond` to become true, polling every 25ms.
    fn wait_for(cond: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            thread::sleep(Duration::from_millis(25));
        }
        false
    }

    #[test]
    fn spawn_and_detect_write() {
        let dir = std::env::temp_dir().join(format!(
            "next-hunk-watch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let w = Watcher::spawn(&dir).expect("spawn watcher");
        // initial state: nothing yet
        assert!(!w.drain());
        // write a file → should observe an event
        fs::write(dir.join("a.txt"), "hi").unwrap();
        assert!(
            wait_for(|| w.drain()),
            "watcher should observe the file write"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn drain_coalesces_multiple_events() {
        let dir = std::env::temp_dir().join(format!(
            "next-hunk-watch2-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let w = Watcher::spawn(&dir).expect("spawn watcher");
        // several rapid writes
        for i in 0..5 {
            fs::write(dir.join(format!("f{i}.txt")), "x").unwrap();
        }
        // drain should report at least one event; a second drain right after
        // may or may not have more, but the coalesced signal is what matters.
        assert!(wait_for(|| w.drain()));
        fs::remove_dir_all(&dir).ok();
    }
}
