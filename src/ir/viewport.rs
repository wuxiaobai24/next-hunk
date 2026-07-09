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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::parse_unified_diff;

    #[test]
    fn viewport_materializes_subset() {
        let sample = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new
";
        let review = parse_unified_diff(sample).unwrap();
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
}
