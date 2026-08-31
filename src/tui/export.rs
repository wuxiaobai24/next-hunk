//! Structured review export on TUI quit.
//!
//! Produces an agent-readable report that is a compatible extension of the
//! existing `--select` / `decision` JSON (`accepted` / `rejected` / `undecided`)
//! plus session comments (same shape as `comment list`) and a banner summary.
//! Formats: JSON, Markdown, or both. Destination: stdout or a file path.
//!
//! This is the human→agent half of the review bridge: the agent points the
//! human at the change (`--focus` / `--note` / `--select`), the human reviews
//! and annotates (`a`/`r`/`u`, `c`), and on quit the whole review outcome
//! lands in one artifact the agent can parse.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::ExportOnQuit;
use crate::tui::app::{CommentEntry, Note, NoteTarget, Selections};

/// Full review report emitted on quit when `export_on_quit` / `--export` is
/// enabled.
///
/// Compatible extension of [`Selections`]: the three decision arrays keep the
/// same names/shape as the `--select` quit output and `next-hunk decision`.
/// Additional fields:
/// - `comments` — same shape as serve `comment list` ([`CommentEntry`]),
///   including the human's `user:N` notes, plus synthetic `note-N` entries for
///   non-banner `--note` agent annotations
/// - `banner` — joined banner-note text, if any
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReviewReport {
    pub accepted: Vec<String>,
    pub rejected: Vec<String>,
    pub undecided: Vec<String>,
    #[serde(default)]
    pub comments: Vec<CommentEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banner: Option<String>,
}

impl ReviewReport {
    /// Project down to the legacy [`Selections`] shape (decision arrays only).
    pub fn as_selections(&self) -> Selections {
        Selections {
            accepted: self.accepted.clone(),
            rejected: self.rejected.clone(),
            undecided: self.undecided.clone(),
        }
    }
}

/// Build a [`ReviewReport`] from decisions, session comments, and notes.
///
/// - Decisions: same bucketing as [`crate::tui::app::App::selections`].
/// - Comments: session [`CommentEntry`] values first, then synthetic entries
///   (`note-0`, `note-1`, …) for `--note` annotations. Notes the human
///   composed with `c` are already mirrored into `comments` as `user:N`, so
///   they are skipped here (matched by target + text) to avoid duplicates.
/// - Banner: non-human banner note texts joined with `"; "`.
pub fn build_report(
    selections: &Selections,
    comments: &[CommentEntry],
    notes: &[Note],
    user_notes: &HashMap<String, Note>,
) -> ReviewReport {
    // Human notes are already mirrored into `comments` as `user:N`, so each
    // mirror consumes exactly ONE matching `notes` entry (target + text).
    // Consuming — rather than matching by value across the whole run — keeps
    // an agent `--note` with byte-identical target and text from being
    // swallowed by its human twin.
    let mut human_mirrors: Vec<&Note> = user_notes.values().collect();

    let mut out_comments = comments.to_vec();
    let mut banner_parts: Vec<&str> = Vec::new();
    let mut note_idx = 0usize;

    for note in notes {
        if let Some(pos) = human_mirrors
            .iter()
            .position(|n| n.target == note.target && n.text == note.text)
        {
            human_mirrors.swap_remove(pos);
            continue;
        }
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

/// Render a Markdown report suitable for pasting into an agent prompt or a PR
/// description.
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
            // Blockquote the body: a multi-line note (serve `comment add`
            // can contain newlines) keeps its line structure, and a line
            // starting with `#`/`-` can't hijack the report's headings.
            for line in c.text.lines() {
                out.push_str("> ");
                out.push_str(line);
                out.push('\n');
            }
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
/// - `none` + `--select`: print the legacy decisions-only JSON (no
///   `comments` / `banner` keys) for backward compatibility.
/// - `none` without `--select`: emit nothing.
/// - `json` / `markdown` / `both`: emit the full report (even without
///   `--select`).
/// - With `export_file`: write to file(s) instead of stdout.
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
                let selections = report.as_selections();
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
    (
        parent.join(format!("{stem}.json")),
        parent.join(format!("{stem}.md")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::NoteTarget;

    fn selections(accepted: &[&str], rejected: &[&str], undecided: &[&str]) -> Selections {
        Selections {
            accepted: accepted.iter().map(|s| s.to_string()).collect(),
            rejected: rejected.iter().map(|s| s.to_string()).collect(),
            undecided: undecided.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn comment(id: &str, file: &str, text: &str, line: Option<u32>) -> CommentEntry {
        CommentEntry {
            id: id.to_string(),
            file: file.to_string(),
            text: text.to_string(),
            line,
            hunk: None,
        }
    }

    #[test]
    fn build_report_combines_decisions_comments_and_notes() {
        let sels = selections(&["a.rs:h1"], &["b.rs:h1"], &["a.rs:h2"]);
        let comments = vec![comment("c0", "a.rs", "session comment", Some(3))];
        let notes = vec![
            Note {
                target: NoteTarget::Banner,
                text: "banner summary".into(),
            },
            Note {
                target: NoteTarget::Line {
                    path: "b.rs".into(),
                    line: 7,
                },
                text: "agent line note".into(),
            },
            Note {
                target: NoteTarget::Hunk {
                    path: "b.rs".into(),
                    hunk: 2,
                },
                text: "agent hunk note".into(),
            },
        ];
        let report = build_report(&sels, &comments, &notes, &HashMap::new());
        assert_eq!(report.accepted, vec!["a.rs:h1"]);
        assert_eq!(report.rejected, vec!["b.rs:h1"]);
        assert_eq!(report.undecided, vec!["a.rs:h2"]);
        assert_eq!(report.banner.as_deref(), Some("banner summary"));
        assert_eq!(report.comments.len(), 3);
        assert_eq!(report.comments[0].id, "c0");
        assert_eq!(report.comments[1].id, "note-0");
        assert_eq!(report.comments[1].line, Some(7));
        assert_eq!(report.comments[2].id, "note-1");
        assert_eq!(report.comments[2].hunk, Some(2));
    }

    #[test]
    fn build_report_dedupes_human_notes_already_in_comments() {
        // A human note composed with `c` is mirrored into `comments` as
        // `user:N` and pushed onto `notes`; the report must not list it twice.
        let human = Note {
            target: NoteTarget::Line {
                path: "a.rs".into(),
                line: 42,
            },
            text: "looks wrong".into(),
        };
        let mut user_notes = HashMap::new();
        user_notes.insert("user:1".to_string(), human.clone());
        let comments = vec![comment("user:1", "a.rs", "looks wrong", Some(42))];
        let notes = vec![
            human,
            Note {
                target: NoteTarget::Line {
                    path: "b.rs".into(),
                    line: 2,
                },
                text: "agent note".into(),
            },
        ];
        let report = build_report(&selections(&[], &[], &[]), &comments, &notes, &user_notes);
        let ids: Vec<&str> = report.comments.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["user:1", "note-0"]);
        assert_eq!(report.comments[0].text, "looks wrong");
        assert_eq!(report.comments[1].file, "b.rs");
    }

    #[test]
    fn build_report_skips_empty_banner() {
        let report = build_report(
            &selections(&[], &[], &[]),
            &[],
            &[Note {
                target: NoteTarget::Banner,
                text: String::new(),
            }],
            &HashMap::new(),
        );
        assert_eq!(report.banner, None);
    }

    #[test]
    fn markdown_report_renders_decisions_and_comments() {
        let report = ReviewReport {
            accepted: vec!["a.rs:h1".into()],
            rejected: vec![],
            undecided: vec!["a.rs:h2".into(), "b.rs:h1".into()],
            comments: vec![
                comment("user:1", "a.rs", "looks wrong", Some(42)),
                comment("note-0", "b.rs", "check this", None),
            ],
            banner: Some("Auth refactor".into()),
        };
        let md = to_markdown(&report);
        assert!(md.starts_with("# next-hunk review report\n"));
        assert!(md.contains("## Banner\n\nAuth refactor"));
        assert!(md.contains("### Accepted\n\n- `a.rs:h1`\n"));
        assert!(md.contains("### Rejected\n\n_(none)_\n"));
        assert!(md.contains("- `a.rs:h2`\n- `b.rs:h1`\n"));
        assert!(md.contains("### `user:1` — a.rs:42\n\n> looks wrong"));
        assert!(md.contains("### `note-0` — b.rs\n\n> check this"));
    }

    #[test]
    fn markdown_body_is_blockquoted_and_multi_line_safe() {
        let report = ReviewReport {
            banner: None,
            comments: vec![
                CommentEntry {
                    id: "c1".into(),
                    file: "a.rs".into(),
                    text: "line one\n# looks like a heading\nline three".into(),
                    line: Some(1),
                    hunk: None,
                },
                CommentEntry {
                    id: "c2".into(),
                    file: String::new(),
                    text: String::new(),
                    line: None,
                    hunk: None,
                },
            ],
            accepted: vec![],
            rejected: vec![],
            undecided: vec![],
        };
        let md = to_markdown(&report);
        // Continuation lines keep their structure and can't hijack headings.
        assert!(md.contains("> line one\n> # looks like a heading\n> line three"));
        assert!(!md.contains("\n# looks like a heading"));
        // An empty body leaves just the heading + blank line: structure intact.
        assert!(md.contains("### `c2` — banner\n\n"));
    }

    #[test]
    fn markdown_report_without_banner_or_comments() {
        let report = ReviewReport {
            accepted: vec![],
            rejected: vec![],
            undecided: vec![],
            comments: vec![],
            banner: None,
        };
        let md = to_markdown(&report);
        assert!(!md.contains("## Banner"));
        assert!(md.contains("## Comments\n\n_(none)_\n"));
    }

    #[test]
    fn emit_none_select_prints_legacy_json_only() {
        let report = ReviewReport {
            accepted: vec!["a.rs:h1".into()],
            rejected: vec![],
            undecided: vec![],
            comments: vec![comment("c0", "a.rs", "hidden", Some(1))],
            banner: Some("hidden".into()),
        };
        // Legacy shape: no comments/banner keys.
        emit_report(&report, ExportOnQuit::None, true, None).unwrap();
        emit_report(&report, ExportOnQuit::None, false, None).unwrap(); // silent
    }

    #[test]
    fn emit_writes_files_with_extension_handling() {
        let tmp = std::env::temp_dir().join(format!("nh-export-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let report = ReviewReport {
            accepted: vec!["a.rs:h1".into()],
            rejected: vec![],
            undecided: vec![],
            comments: vec![comment("c0", "a.rs", "text", Some(1))],
            banner: None,
        };

        // json: existing .json extension is kept as-is.
        let json_path = tmp.join("report.json");
        emit_report(&report, ExportOnQuit::Json, false, Some(&json_path)).unwrap();
        let body = std::fs::read_to_string(&json_path).unwrap();
        assert!(body.starts_with("{\"accepted\":[\"a.rs:h1\"]"));
        assert!(body.contains("\"comments\":["));
        assert!(body.ends_with('\n'));

        // markdown: extension-less path gains .md.
        let md_path = tmp.join("report");
        emit_report(&report, ExportOnQuit::Markdown, false, Some(&md_path)).unwrap();
        assert!(tmp.join("report.md").exists());

        // both: report2.json + report2.md siblings, no double suffix.
        emit_report(
            &report,
            ExportOnQuit::Both,
            false,
            Some(&tmp.join("report2.json")),
        )
        .unwrap();
        assert!(tmp.join("report2.json").exists());
        assert!(tmp.join("report2.md").exists());
        let md_body = std::fs::read_to_string(tmp.join("report2.md")).unwrap();
        assert!(md_body.starts_with("# next-hunk review report"));

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn report_json_round_trips() {
        let report = ReviewReport {
            accepted: vec!["a.rs:h1".into()],
            rejected: vec![],
            undecided: vec![],
            comments: vec![comment("c0", "a.rs", "text", Some(1))],
            banner: Some("b".into()),
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: ReviewReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.accepted, report.accepted);
        assert_eq!(back.comments.len(), 1);
        assert_eq!(back.banner.as_deref(), Some("b"));
    }
}
