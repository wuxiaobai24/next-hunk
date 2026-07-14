//! Engine end-to-end tests: parse the golden fixture patches and assert on
//! the resulting `Review` structure. These exercise the full parse → IR path
//! against hand-crafted edge-case diffs committed under `fixtures/`.

use next_hunk::ir::{parse_unified_diff, DiffLineKind, Viewport, ViewportQuery};

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
