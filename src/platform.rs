//! Platform support surface for live session features.
//!
//! Canonical human-facing matrix: `docs/PLATFORMS.md`.
//! Live session I/O (`serve` / push / decision / list / …) needs Unix domain
//! sockets today; Windows is documented as supported for one-shot review paths
//! only until a future transport (named pipe / localhost TCP) lands.

/// Whether this build can run live session commands (`serve`, `push`, …).
///
/// True only when the `serve` feature is enabled **and** the target is Unix
/// (UDS). Used by docs/tests; CLI gates still use `cfg` for dead-code control.
#[inline]
pub fn live_session_supported() -> bool {
    cfg!(all(feature = "serve", unix))
}

/// Error text when a live-session subcommand cannot run on this build/OS.
///
/// Distinguishes "feature stripped at compile time" (Unix) from "OS has no
/// UDS transport yet" (Windows and other non-Unix), so users are not told to
/// rebuild with `--features serve` when the feature is already on.
pub fn live_session_unavailable(cmd: &str) -> String {
    if cfg!(unix) {
        format!("`{cmd}` requires the `serve` feature (rebuild with --features serve)")
    } else {
        format!(
            "`{cmd}` is unavailable on this OS: live session mode uses Unix \
             domain sockets (Linux/macOS). Windows full serve is deferred to \
             0.9 — see docs/PLATFORMS.md. Alternatives: \
             `next-hunk diff --select`, `next-hunk overlay` (when a TTY/mux is \
             available), or `next-hunk last-export` after a one-shot review."
        )
    }
}

/// Short matrix summary for help text / tests (keep in sync with docs/PLATFORMS.md).
pub const WINDOWS_SUPPORT_SUMMARY: &str = "\
Windows: full for one-shot review (diff/show/pager/inspect/filediff/--select/\
last-export); limited overlay (in-place TTY only, no tmux/zellij); live serve/\
session/MCP tools deferred to 0.9 (Unix domain sockets today).";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_message_names_the_command() {
        let msg = live_session_unavailable("serve");
        assert!(msg.contains("`serve`"), "{msg}");
    }

    #[test]
    fn unavailable_message_is_platform_specific() {
        let msg = live_session_unavailable("push");
        if cfg!(unix) {
            assert!(
                msg.contains("serve") && msg.contains("feature"),
                "unix stub should mention feature rebuild: {msg}"
            );
            assert!(
                !msg.contains("Windows"),
                "unix feature-off path should not talk about Windows: {msg}"
            );
        } else {
            assert!(
                msg.contains("Windows") || msg.contains("docs/PLATFORMS.md"),
                "non-unix should point at platform matrix: {msg}"
            );
            assert!(
                msg.contains("--select") || msg.contains("overlay"),
                "non-unix should suggest one-shot alternatives: {msg}"
            );
        }
    }

    #[test]
    fn live_session_flag_matches_cfg() {
        assert_eq!(live_session_supported(), cfg!(all(feature = "serve", unix)));
    }

    #[test]
    fn windows_summary_mentions_deferral() {
        assert!(WINDOWS_SUPPORT_SUMMARY.contains("Windows"));
        assert!(WINDOWS_SUPPORT_SUMMARY.contains("0.9"));
        assert!(WINDOWS_SUPPORT_SUMMARY.contains("serve"));
    }
}
