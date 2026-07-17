//! Incremental IR rebuild for watch / `reload`.
//!
//! Full reload re-parses every file in a multi-file unified diff even when only
//! one path changed. This module splits the patch into per-file sections,
//! **byte-compares** each section to the previous patch text, and reuses
//! unchanged [`FileDiff`] blocks from the previous [`Review`].
//!
//! ## Arena strategy
//!
//! On the incremental path the previous review is **moved** in (not borrowed),
//! so the shared `text_arena` is reused without re-interning unchanged files.
//! Dirty sections are re-parsed and **appended** to that arena. Stream indices
//! are rewritten in a final linear pass. Dead text from replaced files is left
//! in the arena until a full re-parse compacts it (we force a full parse when
//! more than half the files are dirty, so waste stays bounded).
//!
//! Equality uses raw section memcmp against the previous input string (not a
//! cryptographic hash) so the dirty check stays cheaper than a full re-parse
//! on multi-file reviews.
//!
//! On any structural failure the caller receives the previous review back and
//! should fall back to [`super::parse_unified_diff`].

use std::collections::HashMap;

use super::model::{DiffLine, FileDiff, Hunk, Review};
use super::parse::{parse_unified_diff, ParseError};

/// How a rebuild was performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadMode {
    /// At least one file section was reused from the previous review.
    Incremental,
    /// No reuse (first load, all files dirty, or compacting full parse).
    Full,
}

/// Counters for status / PERF notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalStats {
    pub mode: ReloadMode,
    /// File entries kept from the previous review without re-parse.
    pub reused_files: usize,
    /// File entries produced by re-parsing a dirty section.
    pub reparsed_files: usize,
    pub total_files: usize,
}

/// Result of an incremental (or full) rebuild.
#[derive(Debug, Clone)]
pub struct IncrementalParseResult {
    pub review: Review,
    /// Patch text that produced `review` — kept so the next reload can
    /// byte-compare sections without re-hashing.
    pub source_text: String,
    pub stats: IncrementalStats,
}

/// Error from incremental rebuild. When a previous review was consumed, it is
/// returned so the caller can restore it or discard it for a full parse.
#[derive(Debug)]
pub struct IncrementalError {
    pub error: ParseError,
    /// Previous review if ownership was taken before failure.
    pub previous: Option<Review>,
}

impl std::fmt::Display for IncrementalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error)
    }
}

impl std::error::Error for IncrementalError {}

/// One file's slice of a multi-file unified diff (from `diff --git` through the
/// byte before the next `diff --git`, or the whole input for bare patches).
#[derive(Debug, Clone, Copy)]
pub struct FileSection<'a> {
    pub display_path: &'a str,
    pub text: &'a str,
}

/// Split a unified diff into per-file sections.
///
/// Sections are delimited by `diff --git ` at line starts. When no git header
/// is present but the input looks like a bare `---`/`+++`/`@@` patch, the whole
/// input is returned as a single section.
pub fn split_file_sections(input: &str) -> Vec<FileSection<'_>> {
    let starts = line_start_matches(input, "diff --git ");
    if starts.is_empty() {
        if input.contains("@@ ") || input.contains("--- ") {
            let path = guess_display_path(input);
            return vec![FileSection {
                display_path: path,
                text: input,
            }];
        }
        return Vec::new();
    }

    let mut out = Vec::with_capacity(starts.len());
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(input.len());
        let text = &input[start..end];
        out.push(FileSection {
            display_path: guess_display_path(text),
            text,
        });
    }
    out
}

/// Stable fingerprint of a raw patch section (byte content).
///
/// FNV-1a (64-bit). Useful for tests / diagnostics; the hot reload path prefers
/// direct section memcmp against the previous source text.
pub fn fingerprint_section(text: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut h = FNV_OFFSET;
    for &b in text.as_bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// Full parse path (also used when incremental cannot reuse anything).
pub fn parse_unified_diff_full(input: &str) -> Result<IncrementalParseResult, ParseError> {
    let review = parse_unified_diff(input)?;
    let total = review.file_count();
    Ok(IncrementalParseResult {
        review,
        source_text: input.to_string(),
        stats: IncrementalStats {
            mode: ReloadMode::Full,
            reused_files: 0,
            reparsed_files: total,
            total_files: total,
        },
    })
}

/// Rebuild a [`Review`] from full patch text, reusing unchanged files when
/// `previous_text` section bodies match byte-for-byte.
///
/// `previous` is **consumed** on success. On error it is returned inside
/// [`IncrementalError::previous`] so the caller can restore UI state.
pub fn parse_unified_diff_incremental(
    previous: Option<Review>,
    previous_text: Option<&str>,
    input: &str,
) -> Result<IncrementalParseResult, IncrementalError> {
    if input.is_empty() {
        return Err(IncrementalError {
            error: ParseError::Empty,
            previous,
        });
    }

    // Cheap identical-input path (common when watch debounce fires with no
    // content change): keep the previous IR as-is.
    if let (Some(prev), Some(prev_text)) = (previous.as_ref(), previous_text) {
        if prev_text == input && !prev.files.is_empty() {
            let prev = previous.unwrap();
            let n = prev.file_count();
            return Ok(IncrementalParseResult {
                review: prev,
                source_text: input.to_string(),
                stats: IncrementalStats {
                    mode: ReloadMode::Incremental,
                    reused_files: n,
                    reparsed_files: 0,
                    total_files: n,
                },
            });
        }
    }

    let sections = split_file_sections(input);
    if sections.is_empty() {
        return match parse_unified_diff_full(input) {
            Ok(r) => Ok(r),
            Err(e) => Err(IncrementalError { error: e, previous }),
        };
    }

    let Some(prev) = previous else {
        return match parse_unified_diff_full(input) {
            Ok(r) => Ok(r),
            Err(e) => Err(IncrementalError {
                error: e,
                previous: None,
            }),
        };
    };

    let Some(prev_text) = previous_text else {
        return match parse_unified_diff_full(input) {
            Ok(r) => Ok(r),
            Err(e) => Err(IncrementalError {
                error: e,
                previous: Some(prev),
            }),
        };
    };

    // Map previous section bodies by display path for O(1) equality checks.
    let old_sections = split_file_sections(prev_text);
    let old_body: HashMap<&str, &str> = old_sections
        .iter()
        .map(|s| (s.display_path, s.text))
        .collect();
    let old_file_paths: HashMap<&str, ()> = prev
        .files
        .iter()
        .map(|f| (f.display_path.as_str(), ()))
        .collect();

    struct Plan<'a> {
        sec: FileSection<'a>,
        reuse: bool,
    }
    let mut plan: Vec<Plan<'_>> = Vec::with_capacity(sections.len());
    let mut reusable_count = 0usize;
    let mut dirty_count = 0usize;
    for sec in sections {
        let reuse = old_body
            .get(sec.display_path)
            .is_some_and(|body| *body == sec.text)
            && old_file_paths.contains_key(sec.display_path);
        if reuse {
            reusable_count += 1;
        } else {
            dirty_count += 1;
        }
        plan.push(Plan { sec, reuse });
    }

    // Nothing to reuse, or too dirty → compact full parse.
    if reusable_count == 0 || dirty_count > reusable_count {
        return match parse_unified_diff_full(input) {
            Ok(r) => Ok(r),
            Err(e) => Err(IncrementalError {
                error: e,
                previous: Some(prev),
            }),
        };
    }

    // Move previous arena — no full text re-intern for reused files.
    let mut old_by_path: HashMap<String, FileDiff> = prev
        .files
        .into_iter()
        .map(|f| (f.display_path.clone(), f))
        .collect();

    let dirty_budget: usize = plan
        .iter()
        .filter(|p| !p.reuse)
        .map(|p| p.sec.text.len())
        .sum();
    let mut text_arena = prev.text_arena;
    text_arena.reserve(dirty_budget.saturating_add(1024));

    let mut review = Review {
        text_arena,
        files: Vec::with_capacity(plan.len()),
        stream_len: 0,
        hunk_starts: Vec::new(),
        inserts: 0,
        deletes: 0,
    };

    let mut reused_files = 0usize;
    let mut reparsed_files = 0usize;

    for item in plan {
        if item.reuse {
            if let Some(file) = old_by_path.remove(item.sec.display_path) {
                review.files.push(file);
                reused_files += 1;
            }
        } else {
            match parse_unified_diff(item.sec.text) {
                Ok(partial) => {
                    for f in &partial.files {
                        append_file(&mut review, &partial, f);
                        reparsed_files += 1;
                    }
                }
                Err(ParseError::Empty) | Err(ParseError::NoHunkHeader { .. }) => continue,
            }
        }
    }

    if review.files.is_empty() {
        return match parse_unified_diff(input) {
            Ok(_) => unreachable!("empty files but full parse would succeed inconsistently"),
            Err(e) => Err(IncrementalError {
                error: e,
                previous: None,
            }),
        };
    }

    reindex_streams(&mut review);

    if !stream_invariants_ok(&review) {
        return match parse_unified_diff_full(input) {
            Ok(r) => Ok(r),
            Err(e) => Err(IncrementalError {
                error: e,
                previous: None,
            }),
        };
    }

    let total_files = review.files.len();
    Ok(IncrementalParseResult {
        review,
        source_text: input.to_string(),
        stats: IncrementalStats {
            mode: ReloadMode::Incremental,
            reused_files,
            reparsed_files,
            total_files,
        },
    })
}

/// Append one parsed file's text into `dst.arena` and push a `FileDiff`
/// (stream indices filled later by [`reindex_streams`]).
fn append_file(dst: &mut Review, src: &Review, file: &FileDiff) {
    let mut new_hunks = Vec::with_capacity(file.hunks.len());
    for hunk in &file.hunks {
        let header = push_text(&mut dst.text_arena, src.text(hunk.header.clone()));
        let mut lines = Vec::with_capacity(hunk.lines.len());
        for line in &hunk.lines {
            let text = push_text(&mut dst.text_arena, src.text(line.text.clone()));
            lines.push(DiffLine {
                kind: line.kind,
                text,
            });
        }
        new_hunks.push(Hunk {
            header,
            old_start: hunk.old_start,
            old_count: hunk.old_count,
            new_start: hunk.new_start,
            new_count: hunk.new_count,
            lines,
        });
    }

    let hunk_body: usize = new_hunks.iter().map(|h| 1 + h.lines.len()).sum();
    let min_stream_len = 1 + hunk_body;
    let stream_len = file.stream_len.max(min_stream_len);

    dst.files.push(FileDiff {
        old_path: file.old_path.clone(),
        new_path: file.new_path.clone(),
        display_path: file.display_path.clone(),
        hunks: new_hunks,
        stream_start: 0,
        stream_len,
        inserts: file.inserts,
        deletes: file.deletes,
        origin: file.origin,
    });
}

/// Rewrite `stream_start`, `hunk_starts`, totals after files are assembled.
fn reindex_streams(review: &mut Review) {
    let mut stream_row = 0usize;
    review.hunk_starts.clear();
    review.inserts = 0;
    review.deletes = 0;
    for file in &mut review.files {
        file.stream_start = stream_row;
        let hunk_body: usize = file.hunks.iter().map(|h| 1 + h.lines.len()).sum();
        let structural = 1 + hunk_body;
        if file.stream_len < structural {
            file.stream_len = structural;
        }
        let mut off = 1usize;
        for h in &file.hunks {
            review.hunk_starts.push(stream_row + off);
            off += 1 + h.lines.len();
        }
        review.inserts += file.inserts;
        review.deletes += file.deletes;
        stream_row += file.stream_len;
    }
    review.stream_len = stream_row;
}

fn push_text(arena: &mut String, s: &str) -> std::ops::Range<usize> {
    let start = arena.len();
    arena.push_str(s);
    start..arena.len()
}

fn guess_display_path(section: &str) -> &str {
    // Paths live in the section header block (first ~16 lines). Do not scan the
    // full hunk body — that dominated split cost on huge multi-file patches.
    let mut old: Option<&str> = None;
    let mut new: Option<&str> = None;
    let mut lines_seen = 0usize;

    for raw in section.split_inclusive('\n') {
        lines_seen += 1;
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let line = line.strip_suffix('\r').unwrap_or(line);

        if let Some(rest) = line.strip_prefix("+++ ") {
            new = Some(strip_path_token(rest));
        } else if let Some(rest) = line.strip_prefix("--- ") {
            old = Some(strip_path_token(rest));
        } else if let Some(rest) = line.strip_prefix("diff --git ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 2 {
                old = Some(strip_ab_prefix(parts[0]));
                new = Some(strip_ab_prefix(parts[1]));
            } else if parts.len() == 1 {
                old = Some(strip_ab_prefix(parts[0]));
            }
        } else if line.starts_with("@@ ") {
            // Past headers into hunk body — path is settled.
            break;
        }
        if lines_seen >= 24 {
            break;
        }
    }

    pick_display(new, old)
}

fn pick_display<'a>(new: Option<&'a str>, old: Option<&'a str>) -> &'a str {
    if let Some(p) = new {
        if p != "/dev/null" {
            return p;
        }
    }
    if let Some(p) = old {
        if p != "/dev/null" {
            return p;
        }
    }
    "unknown"
}

fn strip_path_token(rest: &str) -> &str {
    let path = rest.split('\t').next().unwrap_or(rest).trim();
    if path == "/dev/null" {
        return "/dev/null";
    }
    strip_ab_prefix(path)
}

fn strip_ab_prefix(path: &str) -> &str {
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
}

fn line_start_matches(input: &str, needle: &str) -> Vec<usize> {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    for (i, _) in input.match_indices(needle) {
        if i == 0 || bytes[i - 1] == b'\n' {
            out.push(i);
        }
    }
    out
}

fn stream_invariants_ok(review: &Review) -> bool {
    if review.files.is_empty() {
        return review.stream_len == 0;
    }
    let mut expected = 0usize;
    for f in &review.files {
        if f.stream_start != expected {
            return false;
        }
        if f.stream_len == 0 {
            return false;
        }
        expected += f.stream_len;
    }
    if review.stream_len != expected {
        return false;
    }
    for &hs in &review.hunk_starts {
        let ok = review
            .files
            .iter()
            .any(|f| hs > f.stream_start && hs < f.stream_start + f.stream_len);
        if !ok {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{DiffLineKind, Viewport, ViewportQuery};
    use std::collections::HashSet;

    const TWO_FILES: &str = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1 +1 @@
-foo
+bar
";

    const TWO_FILES_A_CHANGED: &str = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+changed
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1 +1 @@
-foo
+bar
";

    #[test]
    fn split_finds_two_git_sections() {
        let secs = split_file_sections(TWO_FILES);
        assert_eq!(secs.len(), 2);
        assert_eq!(secs[0].display_path, "a.rs");
        assert_eq!(secs[1].display_path, "b.rs");
    }

    #[test]
    fn fingerprint_changes_when_body_changes() {
        let a = fingerprint_section(split_file_sections(TWO_FILES)[0].text);
        let b = fingerprint_section(split_file_sections(TWO_FILES_A_CHANGED)[0].text);
        assert_ne!(a, b);
    }

    #[test]
    fn incremental_reuses_unchanged_file() {
        let first = parse_unified_diff_incremental(None, None, TWO_FILES).unwrap();
        assert_eq!(first.stats.mode, ReloadMode::Full);

        let second = parse_unified_diff_incremental(
            Some(first.review),
            Some(&first.source_text),
            TWO_FILES_A_CHANGED,
        )
        .unwrap();
        assert_eq!(second.stats.mode, ReloadMode::Incremental);
        assert_eq!(second.stats.reused_files, 1);
        assert_eq!(second.stats.reparsed_files, 1);

        let full = parse_unified_diff(TWO_FILES_A_CHANGED).unwrap();
        assert_eq!(second.review.stream_len, full.stream_len);
        assert_eq!(second.review.inserts, full.inserts);
    }

    #[test]
    fn incremental_identical_input_is_noop() {
        let first = parse_unified_diff_incremental(None, None, TWO_FILES).unwrap();
        let n = first.review.file_count();
        let second =
            parse_unified_diff_incremental(Some(first.review), Some(&first.source_text), TWO_FILES)
                .unwrap();
        assert_eq!(second.stats.mode, ReloadMode::Incremental);
        assert_eq!(second.stats.reused_files, n);
        assert_eq!(second.stats.reparsed_files, 0);
    }

    #[test]
    fn incremental_matches_full_viewport_rows() {
        let first = parse_unified_diff_incremental(None, None, TWO_FILES).unwrap();
        let src = first.source_text.clone();
        let second =
            parse_unified_diff_incremental(Some(first.review), Some(&src), TWO_FILES_A_CHANGED)
                .unwrap();
        let full = parse_unified_diff(TWO_FILES_A_CHANGED).unwrap();
        let folded = HashSet::new();
        let vp = Viewport {
            start: 0,
            height: 50,
        };
        let rows_inc = ViewportQuery::rows(&second.review, vp, &folded);
        let rows_full = ViewportQuery::rows(&full, vp, &folded);
        assert_eq!(rows_inc.len(), rows_full.len());
    }

    #[test]
    fn empty_input_errors() {
        let err = parse_unified_diff_incremental(None, None, "").unwrap_err();
        assert_eq!(err.error, ParseError::Empty);
    }

    #[test]
    fn binary_section_round_trips() {
        let patch = "\
diff --git a/bin.dat b/bin.dat
index 111..222 100644
Binary files a/bin.dat and b/bin.dat differ
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new
";
        let first = parse_unified_diff_incremental(None, None, patch).unwrap();
        let src = first.source_text.clone();
        let second = parse_unified_diff_incremental(Some(first.review), Some(&src), patch).unwrap();
        assert_eq!(second.stats.reused_files, 2);
        assert_eq!(second.review.files[0].stream_len, 2);
    }

    #[test]
    fn add_delete_line_kinds_preserved_on_reuse() {
        let first = parse_unified_diff_incremental(None, None, TWO_FILES).unwrap();
        let src = first.source_text.clone();
        let second =
            parse_unified_diff_incremental(Some(first.review), Some(&src), TWO_FILES).unwrap();
        let line = &second.review.files[0].hunks[0].lines[0];
        assert_eq!(line.kind, DiffLineKind::Delete);
        assert_eq!(second.review.text(line.text.clone()), "old");
    }

    #[test]
    fn failure_returns_previous_when_possible() {
        let first = parse_unified_diff_incremental(None, None, TWO_FILES).unwrap();
        let src = first.source_text.clone();
        let err = parse_unified_diff_incremental(Some(first.review), Some(&src), "").unwrap_err();
        assert_eq!(err.error, ParseError::Empty);
        assert!(err.previous.is_some());
    }
}
