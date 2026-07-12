//! Parsing for the agent-bridge CLI specs (`--focus` / `--note`).
//!
//! These are kept separate from `main.rs` so the parsing logic is unit-testable
//! in isolation. All functions return [`anyhow::Result`] so a bad spec produces
//! a clear error before the TUI opens.
//!
//! Spec grammar (path is a repo-relative file path):
//! - `--focus <path>`            → `FocusTarget::File`
//! - `--focus <path>:<line>`     → `FocusTarget::FileLine` (`<line>` is a number)
//! - `--focus <path>:h<n>`       → `FocusTarget::FileHunk` (1-based hunk ordinal)
//! - `--note <path>:<line>=<text>`  → `NoteTarget::Line`
//! - `--note <path>:h<n>=<text>`    → `NoteTarget::Hunk`
//! - `--note banner=<text>`         → `NoteTarget::Banner`
//! - `--note =<text>`               → `NoteTarget::Banner` (empty location)

use anyhow::{bail, Result};

use crate::tui::app::{FocusTarget, Note, NoteTarget};

/// Parse a `--focus` spec into a [`FocusTarget`].
pub fn parse_focus(spec: &str) -> Result<FocusTarget> {
    let spec = spec.trim();
    if spec.is_empty() {
        bail!("--focus: empty spec");
    }
    // The rsplit lets file paths contain ':' on systems that allow it; we only
    // treat the last `:segment` as a line/hunk locator.
    let (path, suffix) = match spec.rsplit_once(':') {
        None => return Ok(FocusTarget::File(spec.to_string())),
        Some(pair) => pair,
    };
    if path.is_empty() {
        bail!("--focus: missing path before `:` in `{spec}`");
    }
    if let Some(num) = suffix.strip_prefix('h') {
        let hunk = num
            .parse::<usize>()
            .map_err(|_| anyhow::anyhow!("--focus: invalid hunk ordinal `{num}`"))?;
        if hunk == 0 {
            bail!("--focus: hunk ordinals are 1-based (got h0)");
        }
        Ok(FocusTarget::FileHunk(path.to_string(), hunk))
    } else {
        let line = suffix
            .parse::<u32>()
            .map_err(|_| anyhow::anyhow!("--focus: invalid line number `{suffix}`"))?;
        if line == 0 {
            bail!("--focus: line numbers are 1-based (got 0)");
        }
        Ok(FocusTarget::FileLine(path.to_string(), line))
    }
}

/// Parse one `--note` spec into a [`Note`]. The text portion is everything
/// after the first `=`; a `=` may appear inside the text itself only if the
/// location was `banner` or empty (otherwise the location `key=value` is split
/// on the first `=`).
pub fn parse_note(spec: &str) -> Result<Note> {
    // Split into location and text on the first '='.
    let (location, text) = match spec.split_once('=') {
        Some((loc, text)) => (loc, text),
        None => bail!("--note: missing `=text` in `{spec}`"),
    };
    if location.is_empty() {
        // `--note =text` → banner.
        return Ok(Note {
            target: NoteTarget::Banner,
            text: text.to_string(),
        });
    }
    if location == "banner" {
        return Ok(Note {
            target: NoteTarget::Banner,
            text: text.to_string(),
        });
    }
    // location is `<path>` / `<path>:<line>` / `<path>:h<n>`.
    // A location with a ':' but an empty path half (e.g. `:42=text`) is malformed.
    let (path, suffix) = match location.rsplit_once(':') {
        None => bail!(
            "--note: location `{location}` needs a `:line` or `:h<n>` (use `banner=` for a banner)"
        ),
        Some(pair) => pair,
    };
    if path.is_empty() {
        bail!("--note: missing path before `:` in `{location}`");
    }
    if let Some(num) = suffix.strip_prefix('h') {
        let hunk = num
            .parse::<usize>()
            .map_err(|_| anyhow::anyhow!("--note: invalid hunk ordinal `{num}`"))?;
        if hunk == 0 {
            bail!("--note: hunk ordinals are 1-based (got h0)");
        }
        Ok(Note {
            target: NoteTarget::Hunk {
                path: path.to_string(),
                hunk,
            },
            text: text.to_string(),
        })
    } else {
        let line = suffix
            .parse::<u32>()
            .map_err(|_| anyhow::anyhow!("--note: invalid line number `{suffix}`"))?;
        if line == 0 {
            bail!("--note: line numbers are 1-based (got 0)");
        }
        Ok(Note {
            target: NoteTarget::Line {
                path: path.to_string(),
                line,
            },
            text: text.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_plain_path() {
        assert_eq!(
            parse_focus("src/a.rs").unwrap(),
            FocusTarget::File("src/a.rs".into())
        );
    }

    #[test]
    fn focus_line() {
        assert_eq!(
            parse_focus("src/a.rs:42").unwrap(),
            FocusTarget::FileLine("src/a.rs".into(), 42)
        );
    }

    #[test]
    fn focus_hunk() {
        assert_eq!(
            parse_focus("src/a.rs:h3").unwrap(),
            FocusTarget::FileHunk("src/a.rs".into(), 3)
        );
    }

    #[test]
    fn focus_rejects_zero_line() {
        assert!(parse_focus("a.rs:0").is_err());
    }

    #[test]
    fn focus_rejects_zero_hunk() {
        assert!(parse_focus("a.rs:h0").is_err());
    }

    #[test]
    fn focus_rejects_empty() {
        assert!(parse_focus("").is_err());
    }

    #[test]
    fn note_line() {
        let n = parse_note("a.rs:42=explanation").unwrap();
        assert_eq!(n.target, NoteTarget::Line { path: "a.rs".into(), line: 42 });
        assert_eq!(n.text, "explanation");
    }

    #[test]
    fn note_hunk() {
        let n = parse_note("a.rs:h2=note text").unwrap();
        assert_eq!(n.target, NoteTarget::Hunk { path: "a.rs".into(), hunk: 2 });
        assert_eq!(n.text, "note text");
    }

    #[test]
    fn note_banner_keyword() {
        let n = parse_note("banner=overall summary").unwrap();
        assert_eq!(n.target, NoteTarget::Banner);
        assert_eq!(n.text, "overall summary");
    }

    #[test]
    fn note_banner_empty_location() {
        let n = parse_note("=just text").unwrap();
        assert_eq!(n.target, NoteTarget::Banner);
        assert_eq!(n.text, "just text");
    }

    #[test]
    fn note_text_may_contain_equals() {
        // The text is everything after the FIRST '='.
        let n = parse_note("a.rs:1=key=value").unwrap();
        assert_eq!(n.text, "key=value");
    }

    #[test]
    fn note_rejects_missing_equals() {
        assert!(parse_note("a.rs:1 no equals").is_err());
    }

    #[test]
    fn note_rejects_bare_path() {
        // A bare path with no :line is ambiguous and rejected with guidance.
        assert!(parse_note("a.rs=text").is_err());
    }
}
