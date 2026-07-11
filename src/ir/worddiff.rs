//! Word-level diffing for inline highlight.
//!
//! Produces a token-level diff between two lines so the view can highlight just
//! the words that changed (like `delta`), instead of painting the whole line
//! red/green.
//!
//! Entry points:
//! - [`line_pair_diff`] — diff two strings into `Eq`/`Ins`/`Del` word ops.
//! - [`word_diff_regions`] — split a line into Same/Changed runs relative to a
//!   counterpart, preserving the original text (incl. whitespace). The view
//!   maps these to styles.
//! - [`counterpart_text`] — for a +/- stream row, find the paired line's text
//!   on the other side, so the view knows what to word-diff against.
//! - [`pair_hunk_lines`] — group a hunk's adjacent Delete+Add lines into
//!   pairings (lower-level helper).
//!
//! All pure std (no new dependency). The token-level LCS is a classic
//! Myers-style algorithm over whitespace-split tokens; cheap and viewport-only.

use crate::ir::model::{DiffLine, DiffLineKind, Hunk, Review};
use crate::ir::viewport::ViewportQuery;

/// One token-level operation in a word diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WordOp {
    /// Token present (roughly) equally on both sides.
    Eq(String),
    /// Token only in the new line.
    Ins(String),
    /// Token only in the old line.
    Del(String),
}

/// A run of consecutive Delete lines paired with the run of Add lines that
/// immediately follows them within a hunk. The view uses this to find, for a
/// given Add line, the Delete line(s) it should be word-diffed against.
#[derive(Debug, Clone)]
pub struct Pairing<'a> {
    /// Contiguous Delete lines (old side).
    pub dels: Vec<&'a DiffLine>,
    /// Contiguous Add lines (new side) directly following the deletes.
    pub adds: Vec<&'a DiffLine>,
}

/// Classification of a text run for word-level highlight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordRegion {
    /// Text present (roughly) equally on both sides — render normally.
    Same,
    /// Text present only on our side — the changed part to emphasize.
    Changed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpKind {
    Eq,
    Del,
    Ins,
}

/// Split a line into whitespace-delimited tokens, keeping the tokens but
/// discarding the inter-token whitespace (for diffing purposes). An empty line
/// yields no tokens.
fn tokens(s: &str) -> Vec<&str> {
    s.split_whitespace().collect()
}

/// Like [`tokens`] but also records each token's byte range in `s`, so the
/// view can highlight regions within the original text without losing
/// whitespace or indentation.
fn token_spans(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, ch) in s.char_indices() {
        if ch.is_whitespace() {
            if let Some(st) = start.take() {
                out.push((st, i));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(st) = start {
        out.push((st, s.len()));
    }
    out
}

/// LCS DP + backtrack over two token slices. Returns a sequence of
/// `(OpKind, i, j)` where `i` indexes `a` and `j` indexes `b`. Shared by
/// [`line_pair_diff`] (which maps to `WordOp`) and [`word_diff_regions`]
/// (which maps to positions in the original text).
fn lcs_ops(a: &[&str], b: &[&str]) -> Vec<(OpKind, usize, usize)> {
    let (la, lb) = (a.len(), b.len());

    // LCS DP table over token slices.
    let mut dp = vec![vec![0u32; lb + 1]; la + 1];
    for i in (0..la).rev() {
        for j in (0..lb).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    // Backtrack to produce aligned ops.
    let mut ops = Vec::with_capacity(la + lb);
    let (mut i, mut j) = (0, 0);
    while i < la && j < lb {
        if a[i] == b[j] {
            ops.push((OpKind::Eq, i, j));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            ops.push((OpKind::Del, i, j));
            i += 1;
        } else {
            ops.push((OpKind::Ins, i, j));
            j += 1;
        }
    }
    while i < la {
        ops.push((OpKind::Del, i, j));
        i += 1;
    }
    while j < lb {
        ops.push((OpKind::Ins, i, j));
        j += 1;
    }
    ops
}

/// Compute a word-level diff between `old` and `new`.
///
/// Returns a sequence of [`WordOp`]s. Tokens that are equal (in sequence) are
/// collapsed into `Eq`; tokens only in `old` are `Del`, only in `new` are
/// `Ins`. Uses an LCS over tokens — O(a*b) in token count, which for single
/// code lines is tiny.
pub fn line_pair_diff(old: &str, new: &str) -> Vec<WordOp> {
    let a = tokens(old);
    let b = tokens(new);
    lcs_ops(&a, &b)
        .into_iter()
        .map(|(kind, i, j)| match kind {
            OpKind::Eq => WordOp::Eq(a[i].to_string()),
            OpKind::Del => WordOp::Del(a[i].to_string()),
            OpKind::Ins => WordOp::Ins(b[j].to_string()),
        })
        .collect()
}

/// Split `our_text` into runs, classifying each as [`WordRegion::Same`]
/// (present on both sides) or [`WordRegion::Changed`] (present only on our
/// side). Whitespace between tokens is preserved and classified as `Same`.
///
/// The view uses this to highlight just the changed words within a +/- line,
/// instead of painting the whole line. The full original text is reconstructed
/// in the returned runs (including inter-token whitespace and trailing text),
/// so indentation and formatting are never lost.
pub fn word_diff_regions(our_text: &str, their_text: &str) -> Vec<(WordRegion, String)> {
    let our = token_spans(our_text);
    let their: Vec<&str> = tokens(their_text);
    let our_strs: Vec<&str> = our.iter().map(|(s, e)| &our_text[*s..*e]).collect();

    let ops = lcs_ops(&our_strs, &their);

    let mut runs: Vec<(WordRegion, String)> = Vec::new();
    let mut prev_end = 0usize;

    for (kind, i, _j) in &ops {
        match kind {
            OpKind::Eq | OpKind::Del => {
                let (start, end) = our[*i];
                // Whitespace before this token is always Same.
                if start > prev_end {
                    runs.push((WordRegion::Same, our_text[prev_end..start].to_string()));
                }
                let region = if *kind == OpKind::Eq {
                    WordRegion::Same
                } else {
                    WordRegion::Changed
                };
                runs.push((region, our_text[start..end].to_string()));
                prev_end = end;
            }
            OpKind::Ins => {
                // Token only on their side; not present in our_text — skip.
            }
        }
    }
    // Trailing whitespace / text after the last token.
    if prev_end < our_text.len() {
        runs.push((WordRegion::Same, our_text[prev_end..].to_string()));
    }
    runs
}

/// Group a hunk's body lines into Delete→Add pairings.
///
/// Within a hunk, a maximal run of consecutive `Delete` lines followed by a
/// maximal run of consecutive `Add` lines forms one [`Pairing`]. Context lines
/// and meta lines act as separators (they break pairing). Pure-delete and
/// pure-add runs (one side empty) are still emitted so the view can word-diff
/// against an empty counterpart.
pub fn pair_hunk_lines(hunk: &Hunk) -> Vec<Pairing<'_>> {
    let mut out = Vec::new();
    let mut dels: Vec<&DiffLine> = Vec::new();
    let mut adds: Vec<&DiffLine> = Vec::new();
    for line in &hunk.lines {
        match line.kind {
            DiffLineKind::Delete => {
                // If we were accumulating adds, flush as a (empty-del) pairing.
                if !adds.is_empty() {
                    out.push(Pairing {
                        dels: std::mem::take(&mut dels),
                        adds: std::mem::take(&mut adds),
                    });
                }
                dels.push(line);
            }
            DiffLineKind::Add => {
                adds.push(line);
            }
            _ => {
                // context/meta breaks any in-flight pairing
                if !dels.is_empty() || !adds.is_empty() {
                    out.push(Pairing {
                        dels: std::mem::take(&mut dels),
                        adds: std::mem::take(&mut adds),
                    });
                }
            }
        }
    }
    if !dels.is_empty() || !adds.is_empty() {
        out.push(Pairing { dels, adds });
    }
    out
}

/// For a Delete or Add line at stream row `abs_row`, return the text of its
/// paired counterpart line on the other side (Add for Delete, Delete for Add).
///
/// Within a hunk, a maximal run of Delete lines followed by a maximal run of
/// Add lines forms a pairing; lines are matched by index within the run. If the
/// counterpart run is shorter (e.g. 2 deletes paired with 1 add), the excess
/// lines return `None` — they have no pair to word-diff against.
///
/// Returns `None` for non-+/- lines (context, meta, headers), for unpaired
/// lines, and for rows that can't be located.
pub fn counterpart_text(review: &Review, abs_row: usize) -> Option<String> {
    let (file_idx, line_in_file) = ViewportQuery::file_and_line(review, abs_row)?;
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
            return counterpart_in_hunk(review, hunk, li);
        }
        cursor += hunk.lines.len();
    }
    None
}

/// Find the counterpart text for the line at index `li` within `hunk`.
///
/// Walks the hunk body grouping maximal del-runs + add-runs into pairings
/// (same grouping as [`pair_hunk_lines`], but short-circuits at the target).
fn counterpart_in_hunk(review: &Review, hunk: &Hunk, li: usize) -> Option<String> {
    let target = hunk.lines.get(li)?;
    if !matches!(target.kind, DiffLineKind::Add | DiffLineKind::Delete) {
        return None;
    }

    let mut i = 0;
    while i < hunk.lines.len() {
        // Collect a maximal run of deletes.
        let del_start = i;
        while i < hunk.lines.len() && hunk.lines[i].kind == DiffLineKind::Delete {
            i += 1;
        }
        let del_end = i;
        // Collect a maximal run of adds.
        let add_start = i;
        while i < hunk.lines.len() && hunk.lines[i].kind == DiffLineKind::Add {
            i += 1;
        }
        let add_end = i;

        let n_dels = del_end - del_start;
        let n_adds = add_end - add_start;

        if n_dels == 0 && n_adds == 0 {
            // Separator (context/meta) — skip and continue.
            i += 1;
            continue;
        }

        // Is the target in the del run? → look for a paired add.
        if li >= del_start && li < del_end {
            let idx = li - del_start;
            if idx < n_adds {
                let c = &hunk.lines[add_start + idx];
                return Some(review.text(c.text.clone()).to_string());
            }
            return None; // no counterpart add for this del
        }
        // Is the target in the add run? → look for a paired del.
        if li >= add_start && li < add_end {
            let idx = li - add_start;
            if idx < n_dels {
                let c = &hunk.lines[del_start + idx];
                return Some(review.text(c.text.clone()).to_string());
            }
            return None; // no counterpart del for this add
        }
        // Target not in this pairing; continue scanning.
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::parse_unified_diff;

    // ---- line_pair_diff (existing) ----

    #[test]
    fn identical_lines_all_equal() {
        let ops = line_pair_diff("foo bar baz", "foo bar baz");
        assert!(ops.iter().all(|o| matches!(o, WordOp::Eq(_))));
        assert_eq!(ops.len(), 3);
    }

    #[test]
    fn single_word_change() {
        // "old" → "new" at the start
        let ops = line_pair_diff("old bar", "new bar");
        let has_del = ops.iter().any(|o| matches!(o, WordOp::Del(w) if w == "old"));
        let has_ins = ops.iter().any(|o| matches!(o, WordOp::Ins(w) if w == "new"));
        assert!(has_del, "expected a Del(old): {ops:?}");
        assert!(has_ins, "expected an Ins(new): {ops:?}");
        // "bar" should remain equal
        assert!(ops.iter().any(|o| matches!(o, WordOp::Eq(w) if w == "bar")));
    }

    #[test]
    fn pure_insertion() {
        let ops = line_pair_diff("foo", "foo bar");
        assert!(ops.iter().any(|o| matches!(o, WordOp::Eq(w) if w == "foo")));
        assert!(ops.iter().any(|o| matches!(o, WordOp::Ins(w) if w == "bar")));
        assert!(!ops.iter().any(|o| matches!(o, WordOp::Del(_))));
    }

    #[test]
    fn pure_deletion() {
        let ops = line_pair_diff("foo bar", "foo");
        assert!(ops.iter().any(|o| matches!(o, WordOp::Del(w) if w == "bar")));
        assert!(!ops.iter().any(|o| matches!(o, WordOp::Ins(_))));
    }

    #[test]
    fn empty_strings() {
        let ops = line_pair_diff("", "");
        // both empty → no tokens on either side
        assert!(ops.is_empty());
    }

    #[test]
    fn empty_to_something_is_all_ins() {
        let ops = line_pair_diff("", "hello world");
        assert_eq!(ops.len(), 2);
        assert!(ops.iter().all(|o| matches!(o, WordOp::Ins(_))));
    }

    #[test]
    fn whitespace_only_change_collapses() {
        // split_whitespace treats runs of whitespace equally, so a pure
        // whitespace change yields all-Eq.
        let ops = line_pair_diff("foo   bar", "foo bar");
        assert!(ops.iter().all(|o| matches!(o, WordOp::Eq(_))), "{ops:?}");
    }

    // ---- word_diff_regions ----

    #[test]
    fn regions_identical_lines_all_same() {
        let runs = word_diff_regions("foo bar baz", "foo bar baz");
        assert!(runs.iter().all(|(r, _)| *r == WordRegion::Same), "{runs:?}");
        // Reconstructed text matches original.
        let reconstructed: String = runs.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(reconstructed, "foo bar baz");
    }

    #[test]
    fn regions_single_word_changed() {
        // "old" → "new": our_text = "old bar", their_text = "new bar"
        let runs = word_diff_regions("old bar", "new bar");
        // "old" should be Changed, " bar" should be Same.
        let changed: String = runs
            .iter()
            .filter(|(r, _)| *r == WordRegion::Changed)
            .map(|(_, t)| t.as_str())
            .collect();
        assert_eq!(changed, "old", "changed text should be just 'old': {runs:?}");
        // Full reconstruction preserves original.
        let reconstructed: String = runs.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(reconstructed, "old bar");
    }

    #[test]
    fn regions_preserves_leading_whitespace() {
        // Indentation must survive in a Same run.
        let runs = word_diff_regions("    let x = 1;", "    let y = 1;");
        let reconstructed: String = runs.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(reconstructed, "    let x = 1;");
        // Leading "    " is Same, "x" is Changed.
        assert_eq!(runs[0], (WordRegion::Same, "    ".to_string()));
        // "x" is the changed token.
        assert!(runs.iter().any(|(r, t)| *r == WordRegion::Changed && t == "x"));
    }

    #[test]
    fn regions_pure_insertion_all_changed() {
        // our_text has content, their_text is empty → all our *tokens* are
        // Changed. Inter-token whitespace stays Same (whitespace isn't a word).
        let runs = word_diff_regions("hello world", "");
        let changed_tokens: Vec<&str> = runs
            .iter()
            .filter(|(r, _)| *r == WordRegion::Changed)
            .map(|(_, t)| t.as_str())
            .collect();
        assert_eq!(changed_tokens, vec!["hello", "world"], "{runs:?}");
        // Full text is preserved.
        let reconstructed: String = runs.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(reconstructed, "hello world");
    }

    #[test]
    fn regions_empty_our_text_yields_nothing() {
        let runs = word_diff_regions("", "hello");
        assert!(runs.is_empty());
    }

    #[test]
    fn regions_trailing_whitespace_preserved() {
        let runs = word_diff_regions("foo ", "foo");
        let reconstructed: String = runs.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(reconstructed, "foo ");
    }

    // ---- counterpart_text ----

    /// Stream layout for the paired-review fixture:
    ///   row 0  file header (a.rs)
    ///   row 1  hunk header
    ///   row 2  -old          (del, idx 0)
    ///   row 3  +new value    (add, idx 0)
    fn paired_review() -> Review {
        parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new value
",
        )
        .unwrap()
    }

    #[test]
    fn counterpart_for_paired_del_and_add() {
        let review = paired_review();
        // row 2 = -old, counterpart should be "new value"
        assert_eq!(counterpart_text(&review, 2), Some("new value".to_string()));
        // row 3 = +new value, counterpart should be "old"
        assert_eq!(counterpart_text(&review, 3), Some("old".to_string()));
    }

    #[test]
    fn counterpart_none_for_context_and_headers() {
        let review = paired_review();
        assert_eq!(counterpart_text(&review, 0), None); // file header
        assert_eq!(counterpart_text(&review, 1), None); // hunk header
    }

    #[test]
    fn counterpart_none_for_unpaired_line() {
        // Pure insertion: +new with no preceding -del.
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1,2 @@
 ctx
+new line
",
        )
        .unwrap();
        // row 3 = +new line (no paired delete) → None
        assert_eq!(counterpart_text(&review, 3), None);
    }

    #[test]
    fn counterpart_multi_del_one_add() {
        // 2 deletes paired with 1 add: only the first del (idx 0) gets a
        // counterpart; the second del (idx 1) returns None.
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,2 @@
-foo
-bar
+replacement
",
        )
        .unwrap();
        // stream: 0=header, 1=hunk header, 2=-foo, 3=-bar, 4=+replacement
        // -foo (idx 0) pairs with +replacement
        assert_eq!(counterpart_text(&review, 2), Some("replacement".to_string()));
        // -bar (idx 1) has no counterpart add → None
        assert_eq!(counterpart_text(&review, 3), None);
        // +replacement (idx 0 of adds) pairs with -foo
        assert_eq!(counterpart_text(&review, 4), Some("foo".to_string()));
    }

    #[test]
    fn counterpart_across_context_separator() {
        // A context line between dels and adds breaks the pairing, so the
        // dels have no counterpart.
        let review = parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,3 @@
-old
 ctx
+new
",
        )
        .unwrap();
        // stream: 0=header, 1=hunk header, 2=-old, 3=ctx, 4=+new
        // -old and +new are separated by context → not paired
        assert_eq!(counterpart_text(&review, 2), None);
        assert_eq!(counterpart_text(&review, 4), None);
    }
}
