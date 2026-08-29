//! Context collapsing: the virtual row model.
//!
//! The stream IR keeps every physical row of the unified diff (plus implied
//! unchanged lines *between* hunks, which never appear in the patch text at
//! all). Reviewers do not want to scroll through either. This module derives
//! a flat, virtual-coordinate "segment table" over a [`Review`] where:
//!
//! - long runs of context lines **within** a hunk collapse to one marker row,
//! - the unchanged gap **between** consecutive hunks of a file (implied by
//!   `@@` line numbers, so it works on bare patch input) collapses to one
//!   marker row,
//! - folded files contribute only their header row.
//!
//! Every drawn row is exactly one virtual row — scrolling never walks through
//! invisible space, and `max_scroll` shrinks when content is collapsed or
//! folded. Materialization stays viewport-only: the TUI binary-searches the
//! segment table for the window start and touches only the segments the
//! window overlaps.
//!
//! Markers render as `··· N unchanged lines ···`; jumping to a stream row
//! inside a collapsed run expands just that run
//! ([`CollapseIndex::expand_at_stream`]) so search hits and `--focus` targets
//! are always visible.

use std::collections::HashSet;

use super::model::{DiffLineKind, Review};

/// One segment of the virtual stream: a run of rows that are either all
/// materialized 1:1, or a single marker row standing in for many.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Segment {
    /// A file header row (always one virtual row).
    FileHeader { file_idx: usize, stream: usize },
    /// A hunk header row (always one virtual row).
    HunkHeader {
        file_idx: usize,
        hunk_idx: usize,
        stream: usize,
    },
    /// Unchanged lines implied between two hunks of a file — no stream rows
    /// exist for these; the marker is the only representation. `stream` is
    /// the stream row of the *following* hunk header (the insertion point),
    /// used as the binary-search key.
    Gap {
        count: usize,
        stream: usize,
        file_idx: usize,
    },
    /// A maximal run of context lines *within* one hunk, collapsed to a
    /// single marker row. Carries its location so
    /// [`CollapseIndex::expand_at_stream`] can split it back into lines.
    Run {
        stream: usize,
        count: usize,
        file_idx: usize,
        hunk_idx: usize,
        line_start: usize,
    },
    /// Body lines of one hunk, materialized 1:1. `line_start` indexes into
    /// `hunk.lines` so materialization can resolve text and kind.
    Lines {
        stream: usize,
        count: usize,
        file_idx: usize,
        hunk_idx: usize,
        line_start: usize,
    },
}

impl Segment {
    /// Binary-search key: the stream row this segment starts at (for gaps,
    /// the row they are inserted before).
    fn stream_key(&self) -> usize {
        match self {
            Segment::FileHeader { stream, .. }
            | Segment::HunkHeader { stream, .. }
            | Segment::Gap { stream, .. }
            | Segment::Run { stream, .. }
            | Segment::Lines { stream, .. } => *stream,
        }
    }

    /// Height in virtual rows.
    fn vheight(&self) -> usize {
        match self {
            Segment::FileHeader { .. }
            | Segment::HunkHeader { .. }
            | Segment::Gap { .. }
            | Segment::Run { .. } => 1,
            Segment::Lines { count, .. } => *count,
        }
    }
}

/// Push a 1:1 lines segment if it is non-empty.
fn push_lines(
    segs: &mut Vec<Segment>,
    stream: usize,
    count: usize,
    file_idx: usize,
    hunk_idx: usize,
    line_start: usize,
) {
    if count > 0 {
        segs.push(Segment::Lines {
            stream,
            count,
            file_idx,
            hunk_idx,
            line_start,
        });
    }
}

/// Virtual-coordinate view of a review: which rows exist on screen.
///
/// Build once per review state (load / reload / toggle / fold change) and
/// query per frame. All queries are `O(log n)` via the prefix-summed virtual
/// starts, except `expand_at_stream` which is `O(n)` — it only runs on a
/// human-scale jump into a collapsed run.
#[derive(Debug, Clone, Default)]
pub struct CollapseIndex {
    pub(crate) segs: Vec<Segment>,
    /// Virtual start row of each segment (exclusive prefix sum of heights).
    vstarts: Vec<usize>,
    total_v: usize,
    /// Context-run threshold: runs shorter than this never collapse. `0`
    /// disables collapsing entirely (the index is then a pure 1:1 view that
    /// still honors folds).
    threshold: usize,
}

impl CollapseIndex {
    /// Build the segment table for `review`.
    ///
    /// `threshold` is the minimum number of consecutive context lines (within
    /// a hunk) or implied unchanged lines (between hunks) before a marker row
    /// replaces them; `0` disables markers. `folded` file indices contribute
    /// only their header row.
    pub fn build(review: &Review, threshold: usize, folded: &HashSet<usize>) -> Self {
        let mut segs: Vec<Segment> = Vec::new();
        for (file_idx, file) in review.files.iter().enumerate() {
            segs.push(Segment::FileHeader {
                file_idx,
                stream: file.stream_start,
            });
            if folded.contains(&file_idx) {
                continue;
            }

            let mut prev_old_end: u32 = 0;
            let mut prev_new_end: u32 = 0;
            let mut stream = file.stream_start + 1; // past the file header
            for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
                // Implied gap between the previous hunk and this one. Prefer
                // the old side; a brand-new file has no old side (start 0),
                // so fall back to the new side there.
                let gap = if hunk.old_start > 0 && prev_old_end > 0 {
                    u64::from(hunk.old_start).saturating_sub(u64::from(prev_old_end))
                } else if hunk.new_start > 0 && prev_new_end > 0 {
                    u64::from(hunk.new_start).saturating_sub(u64::from(prev_new_end))
                } else {
                    0
                };
                if threshold > 0 && gap >= threshold as u64 {
                    segs.push(Segment::Gap {
                        count: gap as usize,
                        stream,
                        file_idx,
                    });
                }
                prev_old_end = hunk.old_start.saturating_add(hunk.old_count);
                prev_new_end = hunk.new_start.saturating_add(hunk.new_count);

                segs.push(Segment::HunkHeader {
                    file_idx,
                    hunk_idx,
                    stream,
                });
                stream += 1;

                // Walk the body, splitting out collapsible context runs.
                // `line_start`/`pending` describe the block of visible lines
                // accumulated since the last collapsed run; `run_*` tracks
                // the current context run. Within a hunk, a line's index
                // equals its row offset from `stream`.
                let mut line_start = 0usize;
                let mut pending = 0usize;
                let mut run_len = 0usize;
                let mut run_start = 0usize;
                let close_run = |segs: &mut Vec<Segment>,
                                 line_start: &mut usize,
                                 pending: &mut usize,
                                 run_start: usize,
                                 run_len: usize,
                                 stream_base: usize,
                                 collapsible: bool| {
                    if collapsible {
                        push_lines(
                            segs,
                            stream_base + *line_start,
                            *pending,
                            file_idx,
                            hunk_idx,
                            *line_start,
                        );
                        segs.push(Segment::Run {
                            stream: stream_base + run_start,
                            count: run_len,
                            file_idx,
                            hunk_idx,
                            line_start: run_start,
                        });
                        *line_start = run_start + run_len;
                        *pending = 0;
                    } else {
                        *pending += run_len;
                    }
                };
                for (li, line) in hunk.lines.iter().enumerate() {
                    if line.kind == DiffLineKind::Context {
                        if run_len == 0 {
                            run_start = li;
                        }
                        run_len += 1;
                    } else {
                        if run_len > 0 {
                            close_run(
                                &mut segs,
                                &mut line_start,
                                &mut pending,
                                run_start,
                                run_len,
                                stream,
                                threshold > 0 && run_len >= threshold,
                            );
                            run_len = 0;
                        }
                        pending += 1;
                    }
                }
                if run_len > 0 {
                    close_run(
                        &mut segs,
                        &mut line_start,
                        &mut pending,
                        run_start,
                        run_len,
                        stream,
                        threshold > 0 && run_len >= threshold,
                    );
                }
                push_lines(
                    &mut segs,
                    stream + line_start,
                    pending,
                    file_idx,
                    hunk_idx,
                    line_start,
                );
                stream += hunk.lines.len();
            }
        }

        let mut idx = CollapseIndex {
            segs,
            vstarts: Vec::new(),
            total_v: 0,
            threshold,
        };
        idx.recompute();
        idx
    }

    /// Rebuild `vstarts` / `total_v` from the current segments.
    fn recompute(&mut self) {
        self.vstarts = Vec::with_capacity(self.segs.len());
        let mut v = 0usize;
        for seg in &self.segs {
            self.vstarts.push(v);
            v += seg.vheight();
        }
        self.total_v = v;
    }

    /// Total virtual rows (what `max_scroll` and the status totals use).
    pub fn virtual_len(&self) -> usize {
        self.total_v
    }

    /// The configured threshold (0 = collapsing disabled).
    pub fn threshold(&self) -> usize {
        self.threshold
    }

    /// Segment index containing virtual row `v`, clamped to the last segment.
    fn seg_at_virtual(&self, v: usize) -> usize {
        if self.segs.is_empty() {
            return 0;
        }
        self.vstarts
            .partition_point(|&vs| vs <= v)
            .saturating_sub(1)
            .min(self.segs.len() - 1)
    }

    /// Segment-table accessors for viewport materialization.
    pub(crate) fn segment_count(&self) -> usize {
        self.segs.len()
    }

    pub(crate) fn segment(&self, i: usize) -> &Segment {
        &self.segs[i]
    }

    pub(crate) fn segment_at_virtual(&self, v: usize) -> usize {
        self.seg_at_virtual(v)
    }

    pub(crate) fn vstart_of(&self, i: usize) -> usize {
        self.vstarts[i]
    }

    /// Map a virtual row to the stream row it draws (markers map to the
    /// nearest following real row; collapsed runs map to the run start).
    /// Used by rail sync and the note/match fan-out keying.
    pub fn stream_at_virtual(&self, v: usize) -> usize {
        let i = self.seg_at_virtual(v);
        match &self.segs[i] {
            Segment::Gap { stream, .. } => *stream,
            Segment::Lines { stream, count, .. } => {
                let off = v
                    .saturating_sub(self.vstarts[i])
                    .min(count.saturating_sub(1));
                stream + off
            }
            seg => seg.stream_key(),
        }
    }

    /// Map a stream row to its virtual row. A row inside a collapsed run maps
    /// to the run's marker row (callers that need the row visible should
    /// [`Self::expand_at_stream`] first).
    pub fn virtual_of_stream(&self, row: usize) -> usize {
        let i = self.seg_at_stream(row);
        match &self.segs[i] {
            Segment::Lines { stream, count, .. } => {
                let off = row.saturating_sub(*stream).min(count.saturating_sub(1));
                self.vstarts[i] + off
            }
            _ => self.vstarts[i],
        }
    }

    /// Segment index whose stream extent contains `row` (gaps key to their
    /// following header, which sorts after them for equal keys).
    fn seg_at_stream(&self, row: usize) -> usize {
        if self.segs.is_empty() {
            return 0;
        }
        self.segs
            .partition_point(|s| s.stream_key() <= row)
            .saturating_sub(1)
            .min(self.segs.len() - 1)
    }

    /// If `row` falls inside a collapsed context run, split that run back
    /// into plain lines so the row becomes visible. Returns the run length
    /// that was expanded (0 = nothing to expand at `row`).
    pub fn expand_at_stream(&mut self, row: usize) -> usize {
        let i = self.seg_at_stream(row);
        let Segment::Run {
            stream,
            count,
            file_idx,
            hunk_idx,
            line_start,
        } = self.segs[i]
        else {
            return 0;
        };
        if row < stream || row >= stream + count {
            return 0;
        }
        self.segs[i] = Segment::Lines {
            stream,
            count,
            file_idx,
            hunk_idx,
            line_start,
        };
        self.recompute();
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::parse_unified_diff;
    use std::collections::HashSet as Set;

    /// One file, two hunks 8 unchanged lines apart on the old side:
    /// stream rows: 0=file header, 1=hunk1 header, 2=ctx, 3=-a, 4=+b,
    ///              5=hunk2 header, 6=ctx2, 7=-c, 8=+d   (stream_len 9)
    fn gapped_review() -> Review {
        parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,3 @@
 ctx
-a
+b
@@ -12,3 +12,3 @@
 ctx2
-c
+d
",
        )
        .unwrap()
    }

    /// One file, one hunk with a 12-line context run between the changes.
    /// stream rows: 0=file header, 1=hunk header, 2=-x, 3..14=context, 15=+y
    fn long_context_review() -> Review {
        let body: String = (0..12).map(|i| format!(" pad{i}\n")).collect();
        parse_unified_diff(&format!(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,14 +1,14 @@
-x
{body}+y
"
        ))
        .unwrap()
    }

    #[test]
    fn identity_when_disabled() {
        let review = gapped_review();
        let idx = CollapseIndex::build(&review, 0, &Set::new());
        assert_eq!(idx.virtual_len(), review.stream_len);
        for row in 0..review.stream_len {
            assert_eq!(idx.virtual_of_stream(row), row, "row {row}");
            assert_eq!(idx.stream_at_virtual(row), row);
        }
    }

    #[test]
    fn gap_collapses_to_marker() {
        let review = gapped_review();
        // hunk1 covers old lines 1-3 (end=4); hunk2 starts at old 12 → gap 8.
        let idx = CollapseIndex::build(&review, 8, &Set::new());
        // The gap never occupied stream rows; the marker only adds one.
        assert_eq!(idx.virtual_len(), review.stream_len + 1);
        // virtual: 0 hdr, 1 hunk1 hdr, 2 ctx, 3 -a, 4 +b, 5 marker,
        //          6 hunk2 hdr, 7 ctx2, 8 -c, 9 +d
        assert_eq!(idx.virtual_of_stream(5), 6);
        assert_eq!(idx.virtual_of_stream(6), 7);
        assert_eq!(idx.stream_at_virtual(5), 5); // marker maps to next real row
    }

    #[test]
    fn small_gap_below_threshold_is_invisible() {
        let review = gapped_review();
        let idx = CollapseIndex::build(&review, 9, &Set::new());
        assert_eq!(idx.virtual_len(), review.stream_len);
    }

    #[test]
    fn context_run_collapses_and_expands() {
        let review = long_context_review();
        let idx = CollapseIndex::build(&review, 8, &Set::new());
        // 16 stream rows; the 12-line run collapses to 1 → 16 - 12 + 1 = 5.
        assert_eq!(idx.virtual_len(), 5);
        // virtual: 0 hdr, 1 hunk hdr, 2 -x, 3 marker, 4 +y.
        assert_eq!(idx.virtual_of_stream(2), 2);
        assert_eq!(idx.virtual_of_stream(3), 3);
        assert_eq!(idx.virtual_of_stream(13), 3);
        assert_eq!(idx.virtual_of_stream(15), 4);

        let mut idx = idx;
        assert_eq!(idx.expand_at_stream(10), 12);
        assert_eq!(idx.virtual_len(), review.stream_len);
        for row in 0..review.stream_len {
            assert_eq!(idx.virtual_of_stream(row), row, "post-expand row {row}");
        }
        // Expanding outside a run is a no-op.
        assert_eq!(idx.expand_at_stream(15), 0);
    }

    #[test]
    fn expand_only_the_target_run() {
        // Two separated collapsible runs; expanding one keeps the other.
        let body_a: String = (0..9).map(|i| format!(" a{i}\n")).collect();
        let body_b: String = (0..9).map(|i| format!(" b{i}\n")).collect();
        let review = parse_unified_diff(&format!(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,20 +1,20 @@
-x
{body_a}+mid
{body_b}+y
"
        ))
        .unwrap();
        // rows: 0 hdr, 1 hunk hdr, 2 -x, 3..11 run A (9), 12 +mid,
        //       13..21 run B (9), 22 +y
        let mut idx = CollapseIndex::build(&review, 8, &Set::new());
        assert_eq!(idx.expand_at_stream(15), 9); // inside run B
                                                 // Run A stays collapsed: 23 - (9 - 1) = 15 virtual rows.
        assert_eq!(idx.virtual_len(), review.stream_len - 8);
        assert_eq!(idx.virtual_of_stream(3), 3); // run A marker
                                                 // Rows after run A shift up by 8.
        assert_eq!(idx.virtual_of_stream(15), 7);
    }

    #[test]
    fn folded_file_is_header_only() {
        let review = gapped_review();
        let mut folded = Set::new();
        folded.insert(0);
        let idx = CollapseIndex::build(&review, 0, &folded);
        assert_eq!(idx.virtual_len(), 1);
        assert_eq!(idx.stream_at_virtual(0), 0);
    }

    #[test]
    fn gap_uses_new_side_for_pure_adds() {
        // New file: no old side; hunks 5 unchanged lines apart on the new side.
        let review = parse_unified_diff(
            "\
diff --git a/n.rs b/n.rs
new file mode 100644
--- /dev/null
+++ b/n.rs
@@ -0,0 +1,2 @@
+a
+b
@@ -0,0 +8,2 @@
+c
+d
",
        )
        .unwrap();
        let idx = CollapseIndex::build(&review, 5, &Set::new());
        // gap = 8 - (1+2) = 5 ≥ threshold → one marker inserted.
        assert!(idx.virtual_len() > review.stream_len);
        assert!(idx
            .segs
            .iter()
            .any(|s| matches!(s, Segment::Gap { count: 5, .. })));
    }
}
