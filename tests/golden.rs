//! Engine end-to-end tests: parse the golden fixture patches and assert on
//! the resulting `Review` structure. These exercise the full parse → IR path
//! against hand-crafted edge-case diffs committed under `fixtures/`.

use next_hunk::ir::{parse_unified_diff, CollapseIndex, DiffLineKind, Viewport, ViewportQuery};

fn fixture(name: &str) -> String {
    let path = std::path::Path::new("fixtures").join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
}

#[test]
fn tiny_simple_parses_two_files() {
    let review = parse_unified_diff(&fixture("tiny_simple.patch")).unwrap();
    assert_eq!(review.file_count(), 2);
    assert_eq!(review.files[0].display_path, "src/a.rs");
    assert_eq!(review.files[1].display_path, "src/b.rs");

    // file 0 hunk: context, delete, add, add, context = 5 lines
    let h0 = &review.files[0].hunks[0];
    assert_eq!(h0.lines.len(), 5);
    assert_eq!(h0.lines[0].kind, DiffLineKind::Context);
    assert_eq!(h0.lines[1].kind, DiffLineKind::Delete);
    assert_eq!(h0.lines[2].kind, DiffLineKind::Add);
    assert_eq!(h0.lines[3].kind, DiffLineKind::Add);
    assert_eq!(
        review.text(h0.lines[1].text.clone()),
        "    println!(\"hi\");"
    );

    // stream rows contiguous across the two files
    let f0 = &review.files[0];
    let f1 = &review.files[1];
    assert_eq!(f0.stream_start + f0.stream_len, f1.stream_start);
    assert_eq!(f1.stream_start + f1.stream_len, review.stream_len);
}

#[test]
fn tiny_edge_handles_no_newline_binary_rename() {
    let review = parse_unified_diff(&fixture("tiny_edge.patch")).unwrap();
    // 3 files: no-newline, binary, rename-with-content
    assert_eq!(review.file_count(), 3);

    // file 0: no-newline — delete, meta(no-newline), add, meta(no-newline)
    let f0 = &review.files[0];
    assert_eq!(f0.display_path, "no-newline.txt");
    let h0 = &f0.hunks[0];
    assert_eq!(h0.lines.len(), 4);
    assert_eq!(h0.lines[0].kind, DiffLineKind::Delete);
    assert_eq!(h0.lines[1].kind, DiffLineKind::Meta);
    assert_eq!(h0.lines[2].kind, DiffLineKind::Add);
    assert_eq!(h0.lines[3].kind, DiffLineKind::Meta);
    assert!(review.text(h0.lines[1].text.clone()).contains("No newline"));

    // file 1: binary-only — file header + 1 meta line
    let f1 = &review.files[1];
    assert_eq!(f1.display_path, "bin.dat");
    assert!(f1.hunks.is_empty());
    assert_eq!(f1.stream_len, 2); // header + binary marker

    // file 2: rename — display prefers new path
    let f2 = &review.files[2];
    assert_eq!(f2.display_path, "new.rs");
    assert_eq!(f2.hunks[0].lines.len(), 2); // delete + add
}

#[test]
fn viewport_over_golden_fixture() {
    let review = parse_unified_diff(&fixture("tiny_simple.patch")).unwrap();
    // full stream
    let rows = ViewportQuery::rows(
        &review,
        Viewport {
            start: 0,
            height: review.stream_len,
        },
        &std::collections::HashSet::new(),
    );
    assert_eq!(rows.len(), review.stream_len);
    // first row is a file header, last row is a diff line
    assert!(matches!(
        rows.first().unwrap(),
        next_hunk::ir::StreamRow::FileHeader { .. }
    ));

    // file_at_row agrees with file spans
    let f1_start = review.files[1].stream_start;
    assert_eq!(ViewportQuery::file_at_row(&review, f1_start), Some(1));
}

/// Performance gate: huge fixture parse + viewport must stay within bounds.
///
/// This is a fast CI-friendly smoke gate (not a full bench). It verifies the
/// huge fixture (1.1 MB / 38k lines / ~200 files) can be parsed and a viewport
/// materialized without regression. The PERF.md numbers are tighter and measured
/// via `cargo bench --bench parse` and `cargo bench --bench viewport`; this
/// test is the CI gate (the `perf-gate` workflow job generates the fixture and
/// runs exactly this test) and catches gross regressions with deliberately
/// loose debug-build ceilings — ~30× the observed debug timings so slow CI
/// runners never flake, while an O(n)-blown materialization or a parser
/// regression still trips them.
#[test]
fn huge_fixture_parse_and_viewport_gate() {
    let text = match std::fs::read_to_string("fixtures/huge.patch") {
        Ok(t) if !t.is_empty() => t,
        _ => {
            // Fixture not present — skip gracefully (CI may not generate it).
            eprintln!("warning: fixtures/huge.patch not found; skipping huge fixture gate");
            return;
        }
    };

    // Parse must succeed and produce a reasonable number of files/rows.
    // (Timed: the ceiling below is the gross-regression gate — see the
    // doc comment above.)
    let t_parse = std::time::Instant::now();
    let review = parse_unified_diff(&text).expect("parse huge fixture");
    let parse_elapsed = t_parse.elapsed();
    assert!(review.file_count() > 0, "huge fixture should have files");
    assert!(
        review.stream_len > 1000,
        "huge fixture stream_len should be large, got {}",
        review.stream_len
    );
    assert!(review.inserts > 0, "huge fixture should have insertions");
    assert!(review.deletes > 0, "huge fixture should have deletions");
    assert!(
        !review.hunk_starts.is_empty(),
        "huge fixture should have hunks"
    );

    // Viewport materialization: full stream must produce exactly stream_len rows.
    let full_rows = ViewportQuery::rows(
        &review,
        Viewport {
            start: 0,
            height: review.stream_len,
        },
        &std::collections::HashSet::new(),
    );
    assert_eq!(
        full_rows.len(),
        review.stream_len,
        "full viewport should cover the entire stream"
    );

    // Partial viewport at various positions must produce the expected count.
    for &(start, height) in &[
        (0, 40),
        (500, 40),
        (review.stream_len.saturating_sub(20), 40),
    ] {
        let rows = ViewportQuery::rows(
            &review,
            Viewport { start, height },
            &std::collections::HashSet::new(),
        );
        let expected = height.min(review.stream_len.saturating_sub(start));
        assert_eq!(
            rows.len(),
            expected,
            "viewport at start={start} height={height} should produce {expected} rows"
        );
    }

    // Timed gates (debug build, deliberately loose — see doc comment).
    // Local debug reference: parse ~15 ms, viewport h40 ~0.6 ms.
    assert!(
        parse_elapsed.as_millis() <= 500,
        "parse took {parse_elapsed:?} (>500 ms): gross parser regression"
    );
    let index = CollapseIndex::build(&review, 8, &std::collections::HashSet::new(), false);
    let t_vp = std::time::Instant::now();
    let rows = ViewportQuery::rows_virtual(
        &review,
        Viewport {
            start: index.virtual_len() / 2,
            height: 40,
        },
        &index,
    );
    let vp_elapsed = t_vp.elapsed();
    assert_eq!(rows.len(), 40, "mid-stream viewport should fill 40 rows");
    assert!(
        vp_elapsed.as_millis() <= 50,
        "viewport h40 took {vp_elapsed:?} (>50 ms): materialization blew up"
    );

    // file_at_row must return valid indices for positions within the stream.
    for &row in &[0usize, 100, 1000, review.stream_len.saturating_sub(1)] {
        let fi = ViewportQuery::file_at_row(&review, row);
        assert!(fi.is_some(), "file_at_row({row}) should be Some");
        let fi = fi.unwrap();
        assert!(
            fi < review.file_count(),
            "file_at_row({row}) returned out-of-range index {fi}"
        );
    }

    // file_start_row should be monotonically increasing.
    for i in 1..review.file_count() {
        let prev = ViewportQuery::file_start_row(&review, i - 1);
        let curr = ViewportQuery::file_start_row(&review, i);
        assert!(
            curr >= prev,
            "file_start_row should be non-decreasing: file {i} start {curr} < file {} start {prev}",
            i - 1
        );
    }

    // hunk_starts must be sorted and within stream bounds.
    for &hs in &review.hunk_starts {
        assert!(
            hs < review.stream_len,
            "hunk_start {hs} should be within stream_len {}",
            review.stream_len
        );
    }
    for i in 1..review.hunk_starts.len() {
        assert!(
            review.hunk_starts[i] > review.hunk_starts[i - 1],
            "hunk_starts should be strictly increasing"
        );
    }
}
