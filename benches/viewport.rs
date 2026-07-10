//! Viewport materialization benchmark (PERF.md `viewport_ms`).
//!
//! Parses the huge fixture once, then measures the mean time to materialize
//! one viewport of H rows at many random scroll positions. The Phase 1 gate
//! is mean `< 0.5 ms` for height=40 over 1000 random starts.
//! Run: `cargo bench --bench viewport`.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use next_hunk::ir::{parse_unified_diff, Viewport, ViewportQuery};

fn huge_review_text() -> String {
    match std::fs::read_to_string("fixtures/huge.patch") {
        Ok(t) if !t.is_empty() => t,
        _ => {
            let _ = std::process::Command::new("sh")
                .arg("scripts/gen_fixtures.sh")
                .status();
            std::fs::read_to_string("fixtures/huge.patch").unwrap_or_default()
        }
    }
}

/// Deterministic LCG so bench sample positions are reproducible.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0
    }
}

fn bench_viewport(c: &mut Criterion) {
    let text = huge_review_text();
    if text.is_empty() {
        return;
    }
    let review = parse_unified_diff(&text).unwrap();
    let stream_len = review.stream_len;
    let height = 40usize;

    // Pre-generate 1000 deterministic random starts in [0, stream_len).
    let mut rng = Lcg(0xC0FFEE);
    let starts: Vec<usize> = (0..1000)
        .map(|_| (rng.next() as usize) % stream_len)
        .collect();

    c.bench_function("viewport_huge_h40", |b| {
        b.iter(|| {
            let mut acc = 0usize;
            for &start in black_box(&starts) {
                let rows = ViewportQuery::rows(
                    black_box(&review),
                    Viewport { start, height },
                );
                acc += rows.len();
            }
            black_box(acc);
        })
    });

    // Also a single-viewport measurement for a direct `viewport_ms` number.
    c.bench_function("viewport_single_h40", |b| {
        let start = stream_len / 2;
        b.iter(|| {
            let rows = ViewportQuery::rows(
                black_box(&review),
                Viewport { start, height },
            );
            black_box(rows.len());
        })
    });
}

criterion_group!(benches, bench_viewport);
criterion_main!(benches);
