use std::collections::HashSet;

use super::model::{DiffLineKind, Review};

/// A single virtual row in the flattened multi-file stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamRow<'a> {
    FileHeader {
        file_idx: usize,
        path: &'a str,
    },
    HunkHeader {
        file_idx: usize,
        /// Index of this hunk within its file (0-based). Used by `--select`
        /// decisions and `--note` hunk-level targeting.
        hunk_idx: usize,
        text: &'a str,
    },
    Line {
        file_idx: usize,
        kind: DiffLineKind,
        text: &'a str,
    },
}

/// Visible window into the stream.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub start: usize,
    pub height: usize,
}

impl Viewport {
    pub fn end(&self) -> usize {
        self.start.saturating_add(self.height)
    }
}

pub struct ViewportQuery;

impl ViewportQuery {
    /// Materialize stream rows in `[start, start+height)` without scanning the
    /// whole review when possible (binary search on file spans).
    ///
    /// `folded` is a set of file indices whose bodies (hunks + lines) are
    /// collapsed — only the file header row is emitted.
    pub fn rows<'a>(
        review: &'a Review,
        viewport: Viewport,
        folded: &HashSet<usize>,
    ) -> Vec<StreamRow<'a>> {
        if review.stream_len == 0 || viewport.height == 0 {
            return Vec::new();
        }

        let start = viewport.start.min(review.stream_len.saturating_sub(1));
        let end = viewport.end().min(review.stream_len);
        if start >= end {
            return Vec::new();
        }

        let mut out = Vec::with_capacity(end - start);

        // Find first file that may contribute rows.
        let mut file_idx = review
            .files
            .partition_point(|f| f.stream_start + f.stream_len <= start)
            .min(review.files.len().saturating_sub(1));

        while file_idx < review.files.len() {
            let file = &review.files[file_idx];
            if file.stream_start >= end {
                break;
            }

            let mut row = file.stream_start;

            // File header
            if row >= start && row < end {
                out.push(StreamRow::FileHeader {
                    file_idx,
                    path: file.display_path.as_str(),
                });
            }
            row += 1;

            // When the file is folded, skip its body (hunk headers + lines).
            if folded.contains(&file_idx) {
                file_idx += 1;
                continue;
            }

            for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
                if row >= end {
                    break;
                }
                if row >= start {
                    out.push(StreamRow::HunkHeader {
                        file_idx,
                        hunk_idx,
                        text: review.text(hunk.header.clone()),
                    });
                }
                row += 1;

                for line in &hunk.lines {
                    if row >= end {
                        break;
                    }
                    if row >= start {
                        out.push(StreamRow::Line {
                            file_idx,
                            kind: line.kind,
                            text: review.text(line.text.clone()),
                        });
                    }
                    row += 1;
                }
            }

            file_idx += 1;
        }

        out
    }

    /// Map a stream row index to file index (for rail selection sync).
    pub fn file_at_row(review: &Review, row: usize) -> Option<usize> {
        if review.files.is_empty() {
            return None;
        }
        let idx = review
            .files
            .partition_point(|f| f.stream_start <= row)
            .saturating_sub(1);
        let file = review.files.get(idx)?;
        if row < file.stream_start + file.stream_len {
            Some(idx)
        } else {
            // clamp to last file
            Some(review.files.len() - 1)
        }
    }

    /// Stream row where the given file's header begins.
    ///
    /// Used by the TUI to jump-to-file. Returns the clamped last file start if
    /// the index is out of range (matches `file_at_row` clamping semantics).
    pub fn file_start_row(review: &Review, file_idx: usize) -> usize {
        review
            .files
            .get(file_idx)
            .map(|f| f.stream_start)
            .unwrap_or_else(|| review.files.last().map(|f| f.stream_start).unwrap_or(0))
    }

    /// Advance `file_idx` to the next/previous file, wrapping or clamping.
    /// Returns the new index and its stream start row.
    pub fn jump_file(review: &Review, file_idx: usize, forward: bool) -> Option<(usize, usize)> {
        if review.files.is_empty() {
            return None;
        }
        let n = review.files.len();
        let next = if forward {
            (file_idx + 1) % n
        } else {
            file_idx.checked_sub(1).unwrap_or(n - 1)
        };
        Some((next, Self::file_start_row(review, next)))
    }

    /// Convert an absolute stream row into `(file_idx, line_in_file)`, where
    /// `line_in_file` is the ordinal position of the row *within its file*
    /// (0 = file header, 1 = first hunk header, then body lines).
    ///
    /// Returns `None` for out-of-range rows. Used by the highlight cache key
    /// and by search to map a matched stream row back to a stable identifier.
    pub fn file_and_line(review: &Review, row: usize) -> Option<(usize, usize)> {
        let file_idx = review
            .files
            .partition_point(|f| f.stream_start <= row)
            .saturating_sub(1);
        let file = review.files.get(file_idx)?;
        if row >= file.stream_start + file.stream_len {
            return None;
        }
        let line_in_file = row - file.stream_start;
        Some((file_idx, line_in_file))
    }

    /// The text content of a stream row, if it is a code line (context/add/
    /// delete). File headers and hunk headers return `None`.
    pub fn row_text(review: &Review, row: usize) -> Option<&str> {
        let (file_idx, line_in_file) = Self::file_and_line(review, row)?;
        let file = &review.files[file_idx];
        if line_in_file == 0 {
            return None; // file header
        }
        let mut cursor = 1; // skip file header
        for hunk in &file.hunks {
            if line_in_file == cursor {
                return None; // hunk header
            }
            cursor += 1;
            if line_in_file < cursor + hunk.lines.len() {
                let li = line_in_file - cursor;
                return Some(review.text(hunk.lines[li].text.clone()));
            }
            cursor += hunk.lines.len();
        }
        None
    }

    /// Resolve the (old, new) source line numbers for a stream row, if it is a
    /// code line (context/add/delete). Each side is `None` when the row doesn't
    /// exist on that side (e.g. an `Add` line has no old number).
    ///
    /// Computed by locating the row's hunk within its file and walking from the
    /// hunk's `old_start`/`new_start`, incrementing per line kind. Used by the
    /// line-number gutter and by `e` (open in editor).
    pub fn row_line_numbers(review: &Review, row: usize) -> Option<(Option<u32>, Option<u32>)> {
        use crate::ir::model::DiffLineKind;
        let (file_idx, line_in_file) = Self::file_and_line(review, row)?;
        let file = &review.files[file_idx];
        if line_in_file == 0 {
            return None; // file header
        }
        let mut cursor = 1; // skip file header
        for hunk in &file.hunks {
            if line_in_file == cursor {
                return None; // hunk header
            }
            cursor += 1;
            // Is the target line within this hunk's body?
            if line_in_file < cursor + hunk.lines.len() {
                let li = line_in_file - cursor;
                // Walk [0..=li] accumulating old/new counters from the starts.
                let mut old_no = hunk.old_start;
                let mut new_no = hunk.new_start;
                for (k, line) in hunk.lines.iter().enumerate() {
                    if k == li {
                        return match line.kind {
                            DiffLineKind::Context => Some((Some(old_no), Some(new_no))),
                            DiffLineKind::Add => Some((None, Some(new_no))),
                            DiffLineKind::Delete => Some((Some(old_no), None)),
                            DiffLineKind::Meta => Some((None, None)),
                        };
                    }
                    match line.kind {
                        DiffLineKind::Context => {
                            old_no += 1;
                            new_no += 1;
                        }
                        DiffLineKind::Add => {
                            new_no += 1;
                        }
                        DiffLineKind::Delete => {
                            old_no += 1;
                        }
                        DiffLineKind::Meta => {}
                    }
                }
                return None;
            }
            cursor += hunk.lines.len();
        }
        None
    }

    /// Absolute stream row of the next/previous hunk header relative to `row`,
    /// wrapping across file boundaries.
    ///
    /// * `forward = true`  → first hunk with `hunk_starts[i] > row` (wraps to
    ///   the first hunk if `row` is at or past the last).
    /// * `forward = false` → last hunk with `hunk_starts[i] < row` (wraps to
    ///   the last hunk if `row` is at or before the first).
    ///
    /// Returns `None` only when the review has no hunks (e.g. binary-only files).
    /// Jumping *to* a hunk you're already on is allowed by the caller clamping
    /// `row` away from the exact boundary (see `App::jump_hunk`).
    pub fn jump_hunk(review: &Review, row: usize, forward: bool) -> Option<usize> {
        let starts = &review.hunk_starts;
        if starts.is_empty() {
            return None;
        }
        if forward {
            // partition_point gives the count of elements `<= row`; that index
            // is the first element `> row`, i.e. the next hunk.
            let mut i = starts.partition_point(|&s| s <= row);
            if i >= starts.len() {
                i = 0; // wrap
            }
            Some(starts[i])
        } else {
            // count of elements `< row`; index just before it is the previous hunk.
            let mut i = starts.partition_point(|&s| s < row);
            if i == 0 {
                i = starts.len(); // wrap to last
            }
            i -= 1;
            Some(starts[i])
        }
    }

    /// Resolve a repo-relative path to a file index. Matches against the file's
    /// `display_path`, `new_path`, and `old_path` (in that order) so `--focus`
    /// accepts any of them. Returns `None` when no file matches.
    pub fn file_index_for_path(review: &Review, path: &str) -> Option<usize> {
        review.files.iter().position(|f| {
            f.display_path == path
                || f.new_path.as_deref() == Some(path)
                || f.old_path.as_deref() == Some(path)
        })
    }

    /// Absolute stream row of the `hunk_idx`-th hunk header within `file_idx`.
    /// Walks the file's hunk list accumulating `1 + lines.len()` per hunk,
    /// mirroring how `hunk_starts` is built at parse time (parse.rs). Returns
    /// `None` if `file_idx` or `hunk_idx` is out of range.
    pub fn hunk_start_row(review: &Review, file_idx: usize, hunk_idx: usize) -> Option<usize> {
        let file = review.files.get(file_idx)?;
        let hunk = file.hunks.get(hunk_idx)?;
        // start at the file header row, skip it (+1)
        let mut row = file.stream_start + 1;
        for (k, h) in file.hunks.iter().enumerate() {
            if k == hunk_idx {
                return Some(row);
            }
            // advance past this hunk's header + body
            row += 1 + h.lines.len();
        }
        // unreachable: hunk_idx validated by .get() above
        let _ = hunk;
        None
    }

    /// Absolute stream row of the code line whose *new-side* source line number
    /// equals `line`, within `file_idx`. Used by `--focus path:line` to scroll
    /// to a specific source line. Walks the file's hunks accumulating the
    /// new-side counter from each hunk's `new_start`, matching the reverse of
    /// [`Self::row_line_numbers`]. Returns `None` when no line in the file has
    /// that new-side number (e.g. the line was deleted, or is out of range).
    pub fn row_for_new_line(review: &Review, file_idx: usize, line: u32) -> Option<usize> {
        let file = review.files.get(file_idx)?;
        let mut row = file.stream_start + 1; // skip file header
        for hunk in &file.hunks {
            row += 1; // hunk header
            let mut new_no = hunk.new_start;
            for line_entry in &hunk.lines {
                use crate::ir::model::DiffLineKind;
                if new_no == line
                    && matches!(line_entry.kind, DiffLineKind::Context | DiffLineKind::Add)
                {
                    return Some(row);
                }
                match line_entry.kind {
                    DiffLineKind::Context => {
                        new_no += 1;
                    }
                    DiffLineKind::Add => {
                        new_no += 1;
                    }
                    DiffLineKind::Delete => {}
                    DiffLineKind::Meta => {}
                }
                row += 1;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::parse_unified_diff;

    fn two_file_review() -> Review {
        parse_unified_diff(
            "\
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
",
        )
        .unwrap()
    }

    #[test]
    fn viewport_materializes_subset() {
        let review = two_file_review();
        let rows = ViewportQuery::rows(
            &review,
            Viewport {
                start: 0,
                height: 2,
            },
            &HashSet::new(),
        );
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0], StreamRow::FileHeader { .. }));
    }

    #[test]
    fn viewport_clamps_past_end() {
        let review = two_file_review();
        let total = review.stream_len;
        // A start past the end is clamped to the last row, so we get that row.
        let rows = ViewportQuery::rows(
            &review,
            Viewport {
                start: total + 10,
                height: 5,
            },
            &HashSet::new(),
        );
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn viewport_spanning_two_files() {
        let review = two_file_review();
        let f0_end = review.files[0].stream_start + review.files[0].stream_len;
        // start a couple rows before the boundary, take enough to cross into file 1
        let start = f0_end.saturating_sub(1);
        let rows = ViewportQuery::rows(&review, Viewport { start, height: 4 }, &HashSet::new());
        // should contain a FileHeader for file 1 somewhere
        assert!(rows
            .iter()
            .any(|r| matches!(r, StreamRow::FileHeader { file_idx: 1, .. })));
    }

    #[test]
    fn file_at_row_boundaries() {
        let review = two_file_review();
        let f0 = &review.files[0];
        let f1 = &review.files[1];
        // first row of file 0
        assert_eq!(
            ViewportQuery::file_at_row(&review, f0.stream_start),
            Some(0)
        );
        // last row of file 0
        assert_eq!(
            ViewportQuery::file_at_row(&review, f0.stream_start + f0.stream_len - 1),
            Some(0)
        );
        // first row of file 1
        assert_eq!(
            ViewportQuery::file_at_row(&review, f1.stream_start),
            Some(1)
        );
    }

    #[test]
    fn zero_height_returns_empty() {
        let review = two_file_review();
        let rows = ViewportQuery::rows(
            &review,
            Viewport {
                start: 0,
                height: 0,
            },
            &HashSet::new(),
        );
        assert!(rows.is_empty());
    }

    #[test]
    fn file_start_row_helper() {
        let review = two_file_review();
        assert_eq!(
            ViewportQuery::file_start_row(&review, 0),
            review.files[0].stream_start
        );
        assert_eq!(
            ViewportQuery::file_start_row(&review, 1),
            review.files[1].stream_start
        );
        // out of range clamps to last file start
        assert_eq!(
            ViewportQuery::file_start_row(&review, 99),
            review.files[1].stream_start
        );
    }

    #[test]
    fn jump_file_wraps_and_clamps() {
        let review = two_file_review();
        // forward from last wraps to first
        let (idx, _) = ViewportQuery::jump_file(&review, 1, true).unwrap();
        assert_eq!(idx, 0);
        // backward from first wraps to last
        let (idx, _) = ViewportQuery::jump_file(&review, 0, false).unwrap();
        assert_eq!(idx, 1);
        // forward from first
        let (idx, row) = ViewportQuery::jump_file(&review, 0, true).unwrap();
        assert_eq!(idx, 1);
        assert_eq!(row, review.files[1].stream_start);
    }

    #[test]
    fn empty_review_helpers() {
        let review = Review::default();
        assert_eq!(ViewportQuery::file_at_row(&review, 0), None);
        assert_eq!(ViewportQuery::jump_file(&review, 0, true), None);
        assert_eq!(ViewportQuery::file_start_row(&review, 0), 0);
    }

    // ---- jump_hunk ----

    /// 3 files, 2 hunks each → hunk header rows at:
    ///   file0: rows 1, 5   (header@0; hunk@1 +3 body; hunk@5 +3 body)
    ///   file1: rows 10, 13 (header@9; hunk@10 +2 body; hunk@13 +2 body)
    ///   file2: rows 17, 20 (header@16; hunk@17 +2 body; hunk@20 +2 body)
    fn multi_hunk_review() -> Review {
        parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,2 @@
 ctx
-old1
+new1
@@ -5,2 +5,2 @@
 ctx2
-old2
+new2
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1,1 +1,1 @@
-foo
+bar
@@ -3,1 +3,1 @@
-baz
+qux
diff --git a/c.rs b/c.rs
--- a/c.rs
+++ b/c.rs
@@ -1,1 +1,1 @@
-x
+y
@@ -3,1 +3,1 @@
-p
+q
",
        )
        .unwrap()
    }

    #[test]
    fn jump_hunk_forward_steps_through_all_hunks() {
        let review = multi_hunk_review();
        let starts = review.hunk_starts.clone();
        assert_eq!(starts.len(), 6);
        assert_eq!(starts, vec![1, 5, 10, 13, 17, 20]);
        let mut row = 0usize;
        // forward from the top should land on each hunk header in order.
        for &expected in &starts {
            row = ViewportQuery::jump_hunk(&review, row, true).unwrap();
            assert_eq!(row, expected);
        }
        // one more forward wraps to the first hunk
        let wrapped = ViewportQuery::jump_hunk(&review, row, true).unwrap();
        assert_eq!(wrapped, starts[0]);
    }

    #[test]
    fn jump_hunk_backward_steps_through_all_hunks() {
        let review = multi_hunk_review();
        let starts = review.hunk_starts.clone();
        let mut row = review.stream_len; // past the end
        let mut expected = starts.iter().rev();
        for _ in 0..starts.len() {
            row = ViewportQuery::jump_hunk(&review, row, false).unwrap();
            assert_eq!(row, *expected.next().unwrap());
        }
        // one more backward wraps to the last hunk
        let wrapped = ViewportQuery::jump_hunk(&review, row, false).unwrap();
        assert_eq!(wrapped, *starts.last().unwrap());
    }

    #[test]
    fn jump_hunk_from_mid_body_lands_on_next_header() {
        let review = multi_hunk_review();
        // file0 hunk0 body occupies rows 2,3,4 (ctx, -old1, +new1).
        // forward from row 4 should land on the next hunk header (row 5).
        let next = ViewportQuery::jump_hunk(&review, 4, true).unwrap();
        assert_eq!(next, 5);
        // backward from row 4 lands on the current hunk header (row 1).
        let prev = ViewportQuery::jump_hunk(&review, 4, false).unwrap();
        assert_eq!(prev, 1);
    }

    #[test]
    fn jump_hunk_none_when_no_hunks() {
        let review = Review::default();
        assert!(ViewportQuery::jump_hunk(&review, 0, true).is_none());
        assert!(ViewportQuery::jump_hunk(&review, 0, false).is_none());
    }

    // ---- row_line_numbers ----

    #[test]
    fn row_line_numbers_for_context_add_delete() {
        // hunk: ctx(both), -old(old only), +new(new only)
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -10,3 +10,3 @@
 ctx
-old
+new
",
        )
        .unwrap();
        // stream layout: 0=file header, 1=hunk header, 2=ctx, 3=-old, 4=+new
        // context line is old=10, new=10
        assert_eq!(
            ViewportQuery::row_line_numbers(&review, 2),
            Some((Some(10), Some(10)))
        );
        // -old is old=11, new=None
        assert_eq!(
            ViewportQuery::row_line_numbers(&review, 3),
            Some((Some(11), None))
        );
        // +new is old=None, new=11
        assert_eq!(
            ViewportQuery::row_line_numbers(&review, 4),
            Some((None, Some(11)))
        );
    }

    #[test]
    fn row_line_numbers_none_for_headers() {
        let review = two_file_review();
        // file header (row 0) and hunk header (row 1) → None
        assert_eq!(ViewportQuery::row_line_numbers(&review, 0), None);
        assert_eq!(ViewportQuery::row_line_numbers(&review, 1), None);
    }

    // ---- file_index_for_path / hunk_start_row / row_for_new_line ----

    #[test]
    fn file_index_for_path_matches_display_path() {
        let review = multi_hunk_review();
        assert_eq!(ViewportQuery::file_index_for_path(&review, "a.rs"), Some(0));
        assert_eq!(ViewportQuery::file_index_for_path(&review, "b.rs"), Some(1));
        assert_eq!(ViewportQuery::file_index_for_path(&review, "c.rs"), Some(2));
        assert_eq!(
            ViewportQuery::file_index_for_path(&review, "missing.rs"),
            None
        );
    }

    #[test]
    fn file_index_for_path_empty_review() {
        let review = Review::default();
        assert_eq!(ViewportQuery::file_index_for_path(&review, "a.rs"), None);
    }

    #[test]
    fn hunk_start_row_matches_global_hunk_starts() {
        // Cross-check: hunk_start_row for each (file, hunk) must equal the
        // corresponding entry in the flat global `review.hunk_starts`.
        let review = multi_hunk_review();
        let global = review.hunk_starts.clone();
        // multi_hunk_review layout: 3 files × 2 hunks → global indices 0..6
        // file0 hunks → global[0], global[1]; file1 → global[2], global[3]; etc.
        let mut gi = 0usize;
        for (fi, file) in review.files.iter().enumerate() {
            for hi in 0..file.hunks.len() {
                let row = ViewportQuery::hunk_start_row(&review, fi, hi)
                    .unwrap_or_else(|| panic!("hunk_start_row({fi},{hi}) should be Some"));
                assert_eq!(
                    row, global[gi],
                    "file {fi} hunk {hi} should match global hunk_starts[{gi}]"
                );
                gi += 1;
            }
        }
        assert_eq!(gi, global.len(), "covered every global hunk start");
    }

    #[test]
    fn hunk_start_row_out_of_range() {
        let review = multi_hunk_review();
        // file out of range
        assert_eq!(ViewportQuery::hunk_start_row(&review, 99, 0), None);
        // hunk out of range (file 0 has only 2 hunks)
        assert_eq!(ViewportQuery::hunk_start_row(&review, 0, 99), None);
    }

    #[test]
    fn row_for_new_line_resolves_context_and_add() {
        // Hunk: @@ -10,3 +10,3 @@ with ctx(new=10), -old, +new(new=11).
        // stream: 0=file header, 1=hunk header, 2=ctx, 3=-old, 4=+new.
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -10,3 +10,3 @@
 ctx
-old
+new
",
        )
        .unwrap();
        // new=10 is the context line at row 2
        assert_eq!(ViewportQuery::row_for_new_line(&review, 0, 10), Some(2));
        // new=11 is the add line at row 4
        assert_eq!(ViewportQuery::row_for_new_line(&review, 0, 11), Some(4));
        // new=12 doesn't exist in this hunk
        assert_eq!(ViewportQuery::row_for_new_line(&review, 0, 12), None);
    }

    #[test]
    fn row_for_new_line_skips_delete_lines() {
        // A delete line has no new-side number, so it should never match and
        // must not advance the new counter.
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -5,3 +5,3 @@
 ctx
-deleted
+added
",
        )
        .unwrap();
        // new=5 = ctx @ row 2; new=6 = added @ row 4 (delete @ row 3 skipped)
        assert_eq!(ViewportQuery::row_for_new_line(&review, 0, 5), Some(2));
        assert_eq!(ViewportQuery::row_for_new_line(&review, 0, 6), Some(4));
    }

    #[test]
    fn row_for_new_line_unknown_file() {
        let review = two_file_review();
        assert_eq!(ViewportQuery::row_for_new_line(&review, 99, 1), None);
    }

    // ---- fold ----

    #[test]
    fn rows_skips_body_of_folded_file() {
        let review = two_file_review();
        // file0: stream_start=0, stream_len=4 (header + hunk header + -old + +new)
        // file1: stream_start=4, stream_len=4
        let mut folded = HashSet::new();
        folded.insert(0);
        let rows = ViewportQuery::rows(
            &review,
            Viewport {
                start: 0,
                height: 10,
            },
            &folded,
        );
        // file0 should only emit its header (1 row), file1 should emit all 4 rows
        assert_eq!(rows.len(), 5, "folded file0: 1 header + file1: 4 rows = 5");
        assert!(matches!(rows[0], StreamRow::FileHeader { file_idx: 0, .. }));
        assert!(matches!(rows[1], StreamRow::FileHeader { file_idx: 1, .. }));
        // file1's hunk header and lines should follow
        assert!(matches!(rows[2], StreamRow::HunkHeader { file_idx: 1, .. }));
        assert!(matches!(rows[3], StreamRow::Line { file_idx: 1, .. }));
        assert!(matches!(rows[4], StreamRow::Line { file_idx: 1, .. }));
    }

    #[test]
    fn rows_skips_body_of_all_folded_files() {
        let review = two_file_review();
        let mut folded = HashSet::new();
        folded.insert(0);
        folded.insert(1);
        let rows = ViewportQuery::rows(
            &review,
            Viewport {
                start: 0,
                height: 10,
            },
            &folded,
        );
        // Both files folded: only 2 file header rows
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0], StreamRow::FileHeader { file_idx: 0, .. }));
        assert!(matches!(rows[1], StreamRow::FileHeader { file_idx: 1, .. }));
    }

    #[test]
    fn rows_folded_file_at_viewport_boundary() {
        let review = two_file_review();
        // Viewport starts at file1's header (row 4), file0 is folded.
        // Even though file0 is folded, it's outside the viewport so only file1 appears.
        let mut folded = HashSet::new();
        folded.insert(0);
        let rows = ViewportQuery::rows(
            &review,
            Viewport {
                start: 4,
                height: 4,
            },
            &folded,
        );
        assert_eq!(rows.len(), 4);
        assert!(matches!(rows[0], StreamRow::FileHeader { file_idx: 1, .. }));
    }
}
