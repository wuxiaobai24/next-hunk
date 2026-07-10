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
    pub fn rows<'a>(review: &'a Review, viewport: Viewport) -> Vec<StreamRow<'a>> {
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

            for hunk in &file.hunks {
                if row >= end {
                    break;
                }
                if row >= start {
                    out.push(StreamRow::HunkHeader {
                        file_idx,
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
            .unwrap_or_else(|| {
                review
                    .files
                    .last()
                    .map(|f| f.stream_start)
                    .unwrap_or(0)
            })
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
        );
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn viewport_spanning_two_files() {
        let review = two_file_review();
        let f0_end = review.files[0].stream_start + review.files[0].stream_len;
        // start a couple rows before the boundary, take enough to cross into file 1
        let start = f0_end.saturating_sub(1);
        let rows = ViewportQuery::rows(
            &review,
            Viewport {
                start,
                height: 4,
            },
        );
        // should contain a FileHeader for file 1 somewhere
        assert!(rows.iter().any(|r| matches!(
            r,
            StreamRow::FileHeader {
                file_idx: 1,
                ..
            }
        )));
    }

    #[test]
    fn file_at_row_boundaries() {
        let review = two_file_review();
        let f0 = &review.files[0];
        let f1 = &review.files[1];
        // first row of file 0
        assert_eq!(ViewportQuery::file_at_row(&review, f0.stream_start), Some(0));
        // last row of file 0
        assert_eq!(
            ViewportQuery::file_at_row(&review, f0.stream_start + f0.stream_len - 1),
            Some(0)
        );
        // first row of file 1
        assert_eq!(ViewportQuery::file_at_row(&review, f1.stream_start), Some(1));
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
        );
        assert!(rows.is_empty());
    }

    #[test]
    fn file_start_row_helper() {
        let review = two_file_review();
        assert_eq!(ViewportQuery::file_start_row(&review, 0), review.files[0].stream_start);
        assert_eq!(ViewportQuery::file_start_row(&review, 1), review.files[1].stream_start);
        // out of range clamps to last file start
        assert_eq!(ViewportQuery::file_start_row(&review, 99), review.files[1].stream_start);
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
}
