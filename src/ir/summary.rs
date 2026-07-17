//! Agent-readable review structure summaries (JSON).
//!
//! Shared by `next-hunk inspect --json` (headless, no serve) and the live
//! `next-hunk review` socket reply. Shape is intentionally identical so skills
//! can parse one schema for both paths.

use super::model::Review;

/// A serializable summary of one file in the review, suitable for agent
/// consumption. Contains file paths and hunk metadata but **not** full line
/// content by default (agents request the full patch separately if needed).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct FileSummary {
    pub display_path: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub inserts: u64,
    pub deletes: u64,
    pub hunks: Vec<HunkSummary>,
}

/// A serializable summary of one hunk.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct HunkSummary {
    pub header: String,
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: usize,
}

/// Full file/hunk structure for a review (no full patch text).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ReviewSummary {
    pub file_count: usize,
    pub stream_len: usize,
    pub inserts: u64,
    pub deletes: u64,
    pub files: Vec<FileSummary>,
}

impl From<&Review> for ReviewSummary {
    fn from(review: &Review) -> Self {
        Self {
            file_count: review.file_count(),
            stream_len: review.stream_len,
            inserts: review.inserts,
            deletes: review.deletes,
            files: review
                .files
                .iter()
                .map(|f| FileSummary {
                    display_path: f.display_path.clone(),
                    old_path: f.old_path.clone(),
                    new_path: f.new_path.clone(),
                    inserts: f.inserts,
                    deletes: f.deletes,
                    hunks: f
                        .hunks
                        .iter()
                        .map(|h| HunkSummary {
                            header: review.text(h.header.clone()).to_string(),
                            old_start: h.old_start,
                            old_count: h.old_count,
                            new_start: h.new_start,
                            new_count: h.new_count,
                            lines: h.lines.len(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::parse_unified_diff;

    #[test]
    fn summary_from_tiny_diff() {
        let text = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,3 @@
 context
-old
+new
+more
";
        let review = parse_unified_diff(text).unwrap();
        let summary = ReviewSummary::from(&review);
        assert_eq!(summary.file_count, 1);
        assert_eq!(summary.files[0].display_path, "src/a.rs");
        assert_eq!(summary.files[0].hunks.len(), 1);
        assert!(summary.inserts >= 1);
        // JSON round-trip keeps the public agent shape stable.
        let json = serde_json::to_string(&summary).unwrap();
        let back: ReviewSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(back, summary);
    }
}
