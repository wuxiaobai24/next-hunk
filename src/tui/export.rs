//! Structured review export on TUI quit.
//!
//! Produces an agent-readable report that is a compatible extension of the
//! existing `--select` / `decision` JSON (`accepted` / `rejected` / `undecided`)
//! plus session comments (same shape as `comment list`) and a banner summary.
//!
//! Formats: JSON, Markdown, or both. Destination: stdout or a file path.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::tui::app::{CommentEntry, Note, NoteTarget, ReviewReport, Selections};

// Re-export so callers can use `tui::export::ExportOnQuit`.
pub use crate::config::ExportOnQuit;

/// Build a [`ReviewReport`] from decisions, session comments, and notes.
///
/// - Decisions: same bucketing as [`crate::tui::app::App::selections`].
/// - Comments: session [`CommentEntry`] values first, then non-banner `--note`
///   annotations as synthetic entries with ids `note-0`, `note-1`, …
/// - Banner: all `NoteTarget::Banner` note texts joined with `"; "`.
pub fn build_report(
    selections: &Selections,
    comments: &[CommentEntry],
    notes: &[Note],
) -> ReviewReport {
    let mut out_comments = comments.to_vec();
    let mut banner_parts: Vec<&str> = Vec::new();
    let mut note_idx = 0usize;

    for note in notes {
        match &note.target {
            NoteTarget::Banner => {
                if !note.text.is_empty() {
                    banner_parts.push(note.text.as_str());
                }
            }
            NoteTarget::Line { path, line } => {
                out_comments.push(CommentEntry {
                    id: format!("note-{note_idx}"),
                    file: path.clone(),
                    text: note.text.clone(),
                    line: Some(*line),
                    hunk: None,
                });
                note_idx += 1;
            }
            NoteTarget::Hunk { path, hunk } => {
                out_comments.push(CommentEntry {
                    id: format!("note-{note_idx}"),
                    file: path.clone(),
                    text: note.text.clone(),
                    line: None,
                    hunk: Some(*hunk),
                });
                note_idx += 1;
            }
        }
    }

    let banner = if banner_parts.is_empty() {
        None
    } else {
        Some(banner_parts.join("; "))
    };

    ReviewReport {
        accepted: selections.accepted.clone(),
        rejected: selections.rejected.clone(),
        undecided: selections.undecided.clone(),
        comments: out_comments,
        banner,
    }
}

/// Render a Markdown report suitable for pasting into Claude Code / Codex.
pub fn to_markdown(report: &ReviewReport) -> String {
    let mut out = String::from("# next-hunk review report\n");

    if let Some(banner) = report.banner.as_deref() {
        out.push_str("\n## Banner\n\n");
        out.push_str(banner);
        out.push('\n');
    }

    out.push_str("\n## Decisions\n");
    append_decision_section(&mut out, "Accepted", &report.accepted);
    append_decision_section(&mut out, "Rejected", &report.rejected);
    append_decision_section(&mut out, "Undecided", &report.undecided);

    out.push_str("\n## Comments\n");
    if report.comments.is_empty() {
        out.push_str("\n_(none)_\n");
    } else {
        for c in &report.comments {
            let where_ = match (c.line, c.hunk) {
                (Some(line), _) => format!("{}:{}", c.file, line),
                (None, Some(hunk)) => format!("{}:h{}", c.file, hunk),
                (None, None) => {
                    if c.file.is_empty() {
                        "banner".to_string()
                    } else {
                        c.file.clone()
                    }
                }
            };
            out.push_str(&format!("\n### `{}` — {}\n\n", c.id, where_));
            out.push_str(&c.text);
            out.push('\n');
        }
    }

    out
}

fn append_decision_section(out: &mut String, title: &str, keys: &[String]) {
    out.push_str(&format!("\n### {title}\n\n"));
    if keys.is_empty() {
        out.push_str("_(none)_\n");
    } else {
        for k in keys {
            out.push_str(&format!("- `{k}`\n"));
        }
    }
}

/// Emit the report according to `mode` / `select_mode` / optional file path.
///
/// Rules:
/// - `export_on_quit = none` + `--select`: print legacy `Selections` JSON only
///   (no `comments` / `banner` keys) for backward compatibility.
/// - `export_on_quit = none` without `--select`: emit nothing.
/// - `json` / `markdown` / `both`: emit the full report (even without `--select`).
/// - With `--export-file PATH`: write to file(s) instead of stdout.
///   - `both` → `PATH.json` and `PATH.md` (suffixes replace/append as needed).
pub fn emit_report(
    report: &ReviewReport,
    mode: ExportOnQuit,
    select_mode: bool,
    export_file: Option<&Path>,
) -> Result<()> {
    match mode {
        ExportOnQuit::None => {
            if select_mode {
                let selections = Selections {
                    accepted: report.accepted.clone(),
                    rejected: report.rejected.clone(),
                    undecided: report.undecided.clone(),
                };
                let json = serde_json::to_string(&selections).context("serialize selections")?;
                write_text(export_file, &json, None)?;
            }
            Ok(())
        }
        ExportOnQuit::Json => {
            let json = serde_json::to_string(report).context("serialize review report")?;
            write_text(export_file, &json, Some("json"))?;
            Ok(())
        }
        ExportOnQuit::Markdown => {
            let md = to_markdown(report);
            write_text(export_file, &md, Some("md"))?;
            Ok(())
        }
        ExportOnQuit::Both => {
            let json = serde_json::to_string(report).context("serialize review report")?;
            let md = to_markdown(report);
            match export_file {
                Some(path) => {
                    let (json_path, md_path) = both_paths(path);
                    std::fs::write(&json_path, format!("{json}\n"))
                        .with_context(|| format!("write {}", json_path.display()))?;
                    std::fs::write(&md_path, md)
                        .with_context(|| format!("write {}", md_path.display()))?;
                }
                None => {
                    // JSON first (one line), then a separator, then Markdown.
                    println!("{json}");
                    println!("---");
                    print!("{md}");
                }
            }
            Ok(())
        }
    }
}

/// Write `text` to `path` (adding `ext` when useful) or stdout.
fn write_text(path: Option<&Path>, text: &str, ext: Option<&str>) -> Result<()> {
    match path {
        Some(p) => {
            let target = match ext {
                Some(e) => path_with_ext(p, e),
                None => p.to_path_buf(),
            };
            let body = if text.ends_with('\n') {
                text.to_string()
            } else {
                format!("{text}\n")
            };
            std::fs::write(&target, body)
                .with_context(|| format!("write export file {}", target.display()))?;
        }
        None => {
            // Ensure a trailing newline for stdout consumers.
            if text.ends_with('\n') {
                print!("{text}");
            } else {
                println!("{text}");
            }
        }
    }
    Ok(())
}

fn path_with_ext(path: &Path, ext: &str) -> PathBuf {
    // If the caller already gave the right extension, keep it; otherwise
    // replace or append.
    if path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        e.eq_ignore_ascii_case(ext) || (ext == "md" && e.eq_ignore_ascii_case("markdown"))
    }) {
        return path.to_path_buf();
    }
    let mut out = path.to_path_buf();
    out.set_extension(ext);
    out
}

fn both_paths(path: &Path) -> (PathBuf, PathBuf) {
    // Strip known export extensions so `report.json` → report.json + report.md
    // rather than report.json.json.
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("review-report");
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    // If path has no extension and is a bare name, use it as the stem.
    let base = if path.extension().is_none() {
        path.to_path_buf()
    } else {
        parent.join(stem)
    };
    (path_with_ext(&base, "json"), path_with_ext(&base, "md"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report() -> ReviewReport {
        ReviewReport {
            accepted: vec!["a.rs:h1".into()],
            rejected: vec!["b.rs:h1".into()],
            undecided: vec!["a.rs:h2".into()],
            comments: vec![
                CommentEntry {
                    id: "c0".into(),
                    file: "a.rs".into(),
                    text: "Looks good".into(),
                    line: Some(10),
                    hunk: None,
                },
                CommentEntry {
                    id: "c1".into(),
                    file: "b.rs".into(),
                    text: "Please fix".into(),
                    line: None,
                    hunk: Some(1),
                },
            ],
            banner: Some("Auth refactor".into()),
        }
    }

    #[test]
    fn build_report_merges_notes_and_comments() {
        let selections = Selections {
            accepted: vec!["a.rs:h1".into()],
            rejected: vec![],
            undecided: vec!["a.rs:h2".into()],
        };
        let comments = vec![CommentEntry {
            id: "c0".into(),
            file: "a.rs".into(),
            text: "session comment".into(),
            line: Some(1),
            hunk: None,
        }];
        let notes = vec![
            Note {
                target: NoteTarget::Banner,
                text: "summary one".into(),
            },
            Note {
                target: NoteTarget::Banner,
                text: "summary two".into(),
            },
            Note {
                target: NoteTarget::Line {
                    path: "a.rs".into(),
                    line: 42,
                },
                text: "line note".into(),
            },
            Note {
                target: NoteTarget::Hunk {
                    path: "b.rs".into(),
                    hunk: 2,
                },
                text: "hunk note".into(),
            },
        ];
        let report = build_report(&selections, &comments, &notes);
        assert_eq!(report.accepted, vec!["a.rs:h1"]);
        assert_eq!(report.banner.as_deref(), Some("summary one; summary two"));
        assert_eq!(report.comments.len(), 3);
        assert_eq!(report.comments[0].id, "c0");
        assert_eq!(report.comments[1].id, "note-0");
        assert_eq!(report.comments[1].line, Some(42));
        assert_eq!(report.comments[2].id, "note-1");
        assert_eq!(report.comments[2].hunk, Some(2));
    }

    #[test]
    fn report_json_is_compatible_extension_of_selections() {
        let report = sample_report();
        let json = serde_json::to_string(&report).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.get("accepted").is_some());
        assert!(v.get("rejected").is_some());
        assert!(v.get("undecided").is_some());
        assert!(v.get("comments").is_some());
        assert_eq!(v["banner"], "Auth refactor");
        // Round-trip via Selections-shaped fields.
        let accepted: Vec<String> = serde_json::from_value(v["accepted"].clone()).unwrap();
        assert_eq!(accepted, vec!["a.rs:h1"]);
    }

    #[test]
    fn comment_entry_shape_matches_session_comments() {
        let report = sample_report();
        let json = serde_json::to_string(&report.comments[0]).unwrap();
        let back: CommentEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "c0");
        assert_eq!(back.file, "a.rs");
        assert_eq!(back.line, Some(10));
        assert!(back.hunk.is_none());
        assert_eq!(back.text, "Looks good");
    }

    #[test]
    fn markdown_contains_sections() {
        let md = to_markdown(&sample_report());
        assert!(md.contains("# next-hunk review report"));
        assert!(md.contains("## Banner"));
        assert!(md.contains("Auth refactor"));
        assert!(md.contains("## Decisions"));
        assert!(md.contains("### Accepted"));
        assert!(md.contains("`a.rs:h1`"));
        assert!(md.contains("## Comments"));
        assert!(md.contains("`c0` — a.rs:10"));
        assert!(md.contains("Looks good"));
        assert!(md.contains("`c1` — b.rs:h1"));
    }

    #[test]
    fn export_on_quit_parse() {
        assert_eq!(ExportOnQuit::parse_str("none"), ExportOnQuit::None);
        assert_eq!(ExportOnQuit::parse_str("json"), ExportOnQuit::Json);
        assert_eq!(ExportOnQuit::parse_str("markdown"), ExportOnQuit::Markdown);
        assert_eq!(ExportOnQuit::parse_str("md"), ExportOnQuit::Markdown);
        assert_eq!(ExportOnQuit::parse_str("both"), ExportOnQuit::Both);
        assert_eq!(ExportOnQuit::parse_str("weird"), ExportOnQuit::None);
    }

    #[test]
    fn emit_none_without_select_is_silent() {
        let report = sample_report();
        // Should not panic / write anything to a bogus path when mode is None
        // and select_mode is false.
        emit_report(&report, ExportOnQuit::None, false, None).unwrap();
    }

    #[test]
    fn emit_json_to_file() {
        let dir = std::env::temp_dir().join(format!(
            "next-hunk-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.json");
        let report = sample_report();
        emit_report(&report, ExportOnQuit::Json, false, Some(&path)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["banner"], "Auth refactor");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn emit_both_writes_sibling_files() {
        let dir = std::env::temp_dir().join(format!(
            "next-hunk-export-both-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("report");
        let report = sample_report();
        emit_report(&report, ExportOnQuit::Both, false, Some(&path)).unwrap();
        assert!(dir.join("report.json").is_file());
        assert!(dir.join("report.md").is_file());
        let md = std::fs::read_to_string(dir.join("report.md")).unwrap();
        assert!(md.contains("## Banner"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
