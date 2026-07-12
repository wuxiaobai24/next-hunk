//! Ignore-whitespace view transform.
//!
//! [`strip_whitespace_changes`] produces a copy of a [`Review`] in which a
//! `Delete` line immediately followed by its paired `Add` line — where the two
//! differ *only* in whitespace — is reclassified as `Context`. This mirrors the
//! intent of `git diff -w`: pure formatting churn (indentation, trailing
//! spaces, inner spacing) is collapsed away so only logical changes remain.
//!
//! The transform is pure and runs on the already-parsed IR, so it works for
//! every review source (`diff` / `show` / `patch` / `pager`) without refetching
//! from git. Stream positions and hunk layout are preserved; only line kinds
//! and the derived +/- tallies change. Toggling it back re-runs on the
//! original, so the view is always consistent.

use crate::ir::model::{DiffLine, DiffLineKind, FileDiff, Hunk, Review};

/// `true` if `a` and `b` are equal once all ASCII whitespace is removed.
fn equal_ignoring_whitespace(a: &str, b: &str) -> bool {
    let na: String = a.chars().filter(|c| !c.is_whitespace()).collect();
    let nb: String = b.chars().filter(|c| !c.is_whitespace()).collect();
    na == nb
}

/// Produce a copy of `review` with whitespace-only change pairs collapsed to
/// context. The input is left untouched (toggling re-derives from original).
///
/// Because [`DiffLine`] text is a byte range into the shared arena, we clone the
/// arena so the new review is self-contained.
pub fn strip_whitespace_changes(review: &Review) -> Review {
    let mut out = Review {
        text_arena: review.text_arena.clone(),
        files: Vec::with_capacity(review.files.len()),
        stream_len: review.stream_len,
        hunk_starts: review.hunk_starts.clone(),
        inserts: 0,
        deletes: 0,
    };

    for file in &review.files {
        let mut new_hunks: Vec<Hunk> = Vec::with_capacity(file.hunks.len());
        let mut file_inserts: u64 = 0;
        let mut file_deletes: u64 = 0;
        for hunk in &file.hunks {
            let mut lines: Vec<DiffLine> = hunk.lines.clone();
            collapse_whitespace_runs(&review.text_arena, &mut lines);
            // Recount after reclassification.
            for l in &lines {
                match l.kind {
                    DiffLineKind::Add => file_inserts += 1,
                    DiffLineKind::Delete => file_deletes += 1,
                    _ => {}
                }
            }
            new_hunks.push(Hunk {
                header: hunk.header.clone(),
                old_start: hunk.old_start,
                old_count: hunk.old_count,
                new_start: hunk.new_start,
                new_count: hunk.new_count,
                lines,
            });
        }
        out.inserts += file_inserts;
        out.deletes += file_deletes;
        out.files.push(FileDiff {
            old_path: file.old_path.clone(),
            new_path: file.new_path.clone(),
            display_path: file.display_path.clone(),
            hunks: new_hunks,
            stream_start: file.stream_start,
            stream_len: file.stream_len,
            inserts: file_inserts,
            deletes: file_deletes,
        });
    }
    out
}

/// Collapse whitespace-only `Delete`→`Add` runs in `lines` (text resolved from
/// `arena`) into `Context` lines. Mutates kinds in place; text ranges are left
/// as-is (the context line shows the new-side text, matching git -w output).
fn collapse_whitespace_runs(arena: &str, lines: &mut [DiffLine]) {
    let n = lines.len();
    let mut i = 0;
    while i < n {
        if lines[i].kind != DiffLineKind::Delete {
            i += 1;
            continue;
        }
        let del_start = i;
        while i < n && lines[i].kind == DiffLineKind::Delete {
            i += 1;
        }
        let del_end = i;
        let add_start = i;
        while i < n && lines[i].kind == DiffLineKind::Add {
            i += 1;
        }
        let add_end = i;

        let dels = &lines[del_start..del_end];
        let adds = &lines[add_start..add_end];
        if dels.is_empty() || adds.is_empty() || dels.len() != adds.len() {
            continue;
        }
        // Collapse only if every pair is equal ignoring whitespace.
        let all_match = dels.iter().zip(adds.iter()).all(|(d, a)| {
            let dt = arena.get(d.text.clone()).unwrap_or("");
            let at = arena.get(a.text.clone()).unwrap_or("");
            equal_ignoring_whitespace(dt, at)
        });
        if all_match {
            for l in &mut lines[del_start..del_end] {
                l.kind = DiffLineKind::Context;
            }
            for l in &mut lines[add_start..add_end] {
                l.kind = DiffLineKind::Context;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::parse_unified_diff;

    fn parse(patch: &str) -> Review {
        parse_unified_diff(patch).unwrap()
    }

    #[test]
    fn equal_ignoring_ws_basics() {
        assert!(equal_ignoring_whitespace("  foo bar  ", "foo  bar"));
        assert!(equal_ignoring_whitespace("\tx", "x"));
        assert!(!equal_ignoring_whitespace("foo", "bar"));
        assert!(equal_ignoring_whitespace("", " \t "));
    }

    #[test]
    fn collapses_pure_whitespace_change() {
        // `-  indented` / `+    indented` differ only in spaces → context.
        let patch = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,2 @@
 fn f() {
-  x
+    x
 }
";
        let r = parse(patch);
        let stripped = strip_whitespace_changes(&r);
        let h = &stripped.files[0].hunks[0];
        // Both +/- became context.
        assert!(h.lines.iter().all(|l| l.kind == DiffLineKind::Context));
        assert_eq!(stripped.inserts, 0);
        assert_eq!(stripped.deletes, 0);
    }

    #[test]
    fn keeps_real_changes() {
        let patch = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new
";
        let r = parse(patch);
        let stripped = strip_whitespace_changes(&r);
        let h = &stripped.files[0].hunks[0];
        assert_eq!(h.lines[0].kind, DiffLineKind::Delete);
        assert_eq!(h.lines[1].kind, DiffLineKind::Add);
        assert_eq!(stripped.inserts, 1);
        assert_eq!(stripped.deletes, 1);
    }

    #[test]
    fn mixed_run_not_collapsed() {
        // Run of 2 deletes + 2 adds, but only one pair is whitespace-only →
        // nothing collapses (we collapse whole runs only).
        let patch = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,2 @@
- a
- b
+ a
+ changed
";
        let r = parse(patch);
        let stripped = strip_whitespace_changes(&r);
        // second pair differs in content → whole run stays as +/-.
        assert_eq!(stripped.inserts, 2);
        assert_eq!(stripped.deletes, 2);
    }

    #[test]
    fn original_unchanged() {
        let patch = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-  x
+    x
";
        let r = parse(patch);
        let _ = strip_whitespace_changes(&r);
        // Original still has the +/- lines.
        let h = &r.files[0].hunks[0];
        assert_eq!(h.lines[0].kind, DiffLineKind::Delete);
        assert_eq!(h.lines[1].kind, DiffLineKind::Add);
        assert_eq!(r.inserts, 1);
        assert_eq!(r.deletes, 1);
    }

    #[test]
    fn preserves_layout_positions() {
        // Stream length and hunk_starts must be unchanged so navigation still
        // lands on the right rows.
        let patch = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,3 @@
 ctx
-  x
+    x
 ctx2
";
        let r = parse(patch);
        let before_len = r.stream_len;
        let before_starts = r.hunk_starts.clone();
        let stripped = strip_whitespace_changes(&r);
        assert_eq!(stripped.stream_len, before_len);
        assert_eq!(stripped.hunk_starts, before_starts);
    }
}
