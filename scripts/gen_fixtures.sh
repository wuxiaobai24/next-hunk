#!/usr/bin/env bash
# Deterministic unified-diff fixture generator for next-hunk.
#
# Produces tiers under fixtures/:
#   small  ~3 files   ~200 diff lines
#   medium ~50 files  ~8k diff lines
#   huge   ~200 files ~50k+ diff lines
#
# Pure shell (awk) — no extra runtime deps. Deterministic via a seeded
# LCG so benches do not drift. Output is a valid unified diff suitable
# for `parse_unified_diff` and `git apply`.
#
# Usage: ./scripts/gen_fixtures.sh   (regenerates fixtures/{small,medium,huge}.patch)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/fixtures"
mkdir -p "$OUT"

# Seeded LCG awk snippet: rand sequence from a seed, reproducible.
gen_tier() {
  local files=$1 lines=$2 changed=$3 seed=$4 dest=$5
  awk -v files="$files" -v lines="$lines" -v changed="$changed" -v seed="$seed" '
    BEGIN {
      # LCG parameters (Numerical Recipes)
      s = seed
      for (f = 1; f <= files; f++) {
        path = sprintf("src/module_%04d.rs", f)
        printf "diff --git a/%s b/%s\n", path, path
        printf "index %07x..%07x 100644\n", (s % 0xfffffff), ((s * 1103515245 + 12345) % 0xfffffff)
        printf "--- a/%s\n", path
        printf "+++ b/%s\n", path
        # pick a hunk window start
        hunk_start = 1 + (s % (lines > changed ? lines - changed : 1))
        s = (s * 1103515245 + 12345) % 0x80000000
        # context before
        ctx_before = 3
        ctx_after = 3
        total = changed + ctx_before + ctx_after
        old_count = total
        new_count = total + 1   # net add one line per change block
        printf "@@ -%d,%d +%d,%d @@\n", hunk_start, old_count, hunk_start, new_count
        for (i = 0; i < ctx_before; i++) {
          printf " context_line_%d\n", (hunk_start + i)
        }
        for (i = 0; i < changed; i++) {
          s = (s * 1103515245 + 12345) % 0x80000000
          printf "-    let value = %d;\n", s
          s = (s * 1103515245 + 12345) % 0x80000000
          printf "+    let value = %d;\n", s
          printf "+    // added assertion for value\n"
        }
        for (i = 0; i < ctx_after; i++) {
          printf " context_line_%d\n", (hunk_start + ctx_before + i)
        }
      }
    }
  ' > "$dest"
}

echo "generating small..."
gen_tier 3   80   20  42 "$OUT/small.patch"
echo "generating medium..."
gen_tier 50  200  40  42 "$OUT/medium.patch"
echo "generating huge..."
gen_tier 200 400  60  42 "$OUT/huge.patch"

echo "done."
for t in small medium huge; do
  lc=$(wc -l < "$OUT/$t.patch")
  fc=$(grep -c '^diff --git' "$OUT/$t.patch")
  printf "  %-6s files=%-4d lines=%d\n" "$t" "$fc" "$lc"
done
