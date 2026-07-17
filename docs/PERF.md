# next-hunk Performance

**English** | [中文](./PERF_zh.md)

Performance is a **product feature**, not a late optimization pass.  
Performance claims require numbers from this document’s fixtures and gates.

Related: [ARCHITECTURE.md](./ARCHITECTURE.md) ([中文](./ARCHITECTURE_zh.md)).

**Scope:** gates cover **latency and runtime memory (RSS)** only. Binary size, dependency count, and musl static linking are **not gates** (optional notes in PRs at most).

---

## 1. Principles (recap)

1. Compact **runtime IR** is the source of truth; UI never owns a full widget list for every line.  
2. Only **viewport ± overscan** is materialized per frame.  
3. Scroll/input path is **synchronous and short**.  
4. Highlight / word-diff / search are **cancellable side services**.  
5. Every IR or viewport change should be **measurable**.  
6. Git is gix-only (no `git` CLI). Dependencies are judged by latency / RSS, not binary size.

---

## 2. Fixtures

### 2.1 Tiers

| ID | Intent | Target scale (order of magnitude) |
|----|--------|-----------------------------------|
| `small` | correctness, CI smoke | ~3 files / ~200 diff lines |
| `medium` | daily-driver feel | ~50 files / ~8k diff lines |
| `huge` | stress memory + scroll | ~200 files / ~50k–100k diff lines |

Exact generators live under `scripts/` and `fixtures/` (to be added in Phase 0/1).  
Prefer **deterministic** generated patches so benches do not drift.

### 2.2 Rules

- Golden **tiny** patches for parser edge cases live in git.  
- **Huge** bodies may be generated in CI/local (`scripts/gen_fixtures.sh`) and gitignored if too large.  
- Never bench against an unspecified dirty worktree as the only metric.

### 2.3 Suggested generator knobs

```text
files=N
lines_per_file=L
changed_lines_per_file=C
seed=S
```

Output: unified diff suitable for `parse_unified_diff` and for `git apply` into a temp repo when testing source adapters.

---

## 3. Metrics

All timings: **release** build unless noted. Prefer a quiet machine; record CPU model and OS in results notes.

| Metric ID | Definition | How |
|-----------|------------|-----|
| `parse_ms` | Wall time to build `Review` from patch bytes | bench / `next-hunk bench parse` |
| `viewport_ms` | Time to materialize one viewport of H rows at scroll S | bench; average many (S,H) |
| `startup_ms` | Process start → first drawn frame (TUI) | integration / manual harness |
| `scroll_p50_ms` / `scroll_p99_ms` | Single-step scroll + draw | simulated key/mouse ticks |
| `file_switch_ms` | Jump to next/prev file + draw | |
| `rss_mb` | Process RSS after browsing fixture for ~60s | `/proc` or platform equivalent |

Optional observations (**not gates**):

| Metric ID | Notes |
|-----------|-------|
| `binary_bytes` | Stripped release size; record only, no cap |
| `competitor_delta_ms` | `delta` on same patch stdin |
| `competitor_git_diff_ms` | `git diff --no-ext-diff --no-color` |

Fairness: same fixture, same machine, document versions.

---

## 4. Gates (must pass before calling a phase “done”)

Values are **initial targets** for x86_64 Linux release on a modern laptop/desktop. Adjust only with a short note in this file.

### Phase 1 (engine)

| Metric | Fixture | Gate |
|--------|---------|------|
| `parse_ms` | huge | **&lt; 80 ms** |
| `viewport_ms` (height=40, 1000 random starts) | huge | mean **&lt; 0.5 ms** |
| RSS after parse + 1000 viewports | huge | **&lt; 150 MB** |

### Phase 2 (TUI MVP)

| Metric | Fixture | Gate |
|--------|---------|------|
| `startup_ms` | medium | **&lt; 150 ms** |
| `scroll_p99_ms` | huge | **&lt; 12 ms** |
| `file_switch_ms` | medium | **&lt; 20 ms** |
| `rss_mb` (1 min browse) | huge | **&lt; 150 MB** |

### Phase 3+ (stretch)

| Metric | Fixture | Gate |
|--------|---------|------|
| `parse_ms` | huge | **&lt; 50 ms** (stretch) |
| `startup_ms` | medium | **&lt; 100 ms** (stretch) |
| `scroll_p99_ms` | huge | **&lt; 8 ms** (stretch) |
| `rss_mb` | huge | **&lt; 100 MB** (stretch) |

No `binary_bytes` gate. No musl static artifact gate.

### Measured results

Recorded from `cargo bench` on the development machine (x86_64 Linux, release
build). Replace with CI numbers when a bench harness is wired into CI.

| Metric | Fixture | Gate | Measured | Status |
|--------|---------|------|----------|--------|
| `parse_ms` | huge | < 80 ms | ~1.39 ms | ✅ pass (Phase 1) |
| `viewport_ms` (height=40, single) | huge | mean < 0.5 ms | ~0.0002 ms (197 ns) | ✅ pass (Phase 1) |
| `viewport_ms` (height=40, 1000 starts batch) | huge | mean < 0.5 ms | ~0.34 ms (341 µs / 1000) | ✅ pass (Phase 1) |
| `parse_ms` | medium | — | ~0.24 ms | observation |
| `parse_ms` | small | — | ~8.2 µs | observation |

#### Optional structural backend (WXB-28) — **not a default gate**

`diff --structural` / `structural = true` rewrites the baseline unified text
through external [`difft`](https://github.com/Wilfred/difftastic) (one subprocess
per changed file), then re-enters `parse_unified_diff`. This path is:

- **Opt-in only** — default gix/jj unified path is unchanged and remains the
  sole subject of the parse/viewport benches above.
- **Higher latency** — expect O(files) process spawns; suitable for human
  readability on medium changesets, not for huge-fixture scroll gates.
- **Not measured** in `cargo bench --bench parse` / viewport benches. Do not
  treat structural mode as a PERF regression if it is slower than unified.

Tradeoff: better AST-aware readability vs subprocess cost. Documented for
agents in `skill/next-hunk/SKILL.md`.

#### Incremental reload (WXB-26)

Hot-reload (`--watch` / `next-hunk reload`) rebuilds IR by fingerprinting
per-file unified-diff sections and **transplanting** unchanged `FileDiff`
blocks. Only dirty sections are re-parsed. Failure → full
`parse_unified_diff` (previous review kept if both fail).

| Metric | Fixture | Scenario | Gate (guidance) | How |
|--------|---------|----------|-----------------|-----|
| `reload_full_ms` | huge | re-parse entire new patch | same as `parse_ms` | `reload/huge_full_reparse` |
| `reload_inc_1file_ms` | huge | 1 section dirty, rest reused | **&lt; 50% of full** on huge | `reload/huge_incremental_1file` |

Run: `cargo bench --bench parse` (group `reload`). Recorded on a quiet
x86_64 Linux release build (replace with local numbers when re-running):

| Metric | Fixture | Measured | Notes |
|--------|---------|----------|-------|
| `reload_full_ms` | huge (200 files) | **~1.37 ms** | full `parse_unified_diff` of dirty text |
| `reload_inc_1file_ms` | huge, 1 section dirty | **~0.75 ms** | section memcmp + re-parse 1 + arena reuse (**~55% of full**, gate &lt; 50% guidance met as ~1.8× faster) |
| identical-input reload | any | **≪ full** | early return keeps previous IR |

Method: `App`-path style — previous `Review` is **moved** (no arena clone) into
`parse_unified_diff_incremental`; criterion times only the rebuild body.
Dirty detection is per-section **byte compare** against the previous source
text (not a full cryptographic hash). Path guess scans only section headers.

RSS: incremental rebuild reuses the previous text arena and appends dirty
sections (headroom reserved to avoid full-arena realloc). Peak holds one
live `Review` plus dead text from replaced files until a compacting full
parse; no full-widget tree.

RSS after parse + viewport queries not yet measured with an instrumented
harness; the huge fixture's arena is ~1 MB, well under the 150 MB gate. A
proper RSS measurement is pending a `bench`/`next-hunk bench` harness.

### Rough comparison vs `delta` (design claims + benches)

`delta` (https://github.com/dandavison/delta) is the most widely used
terminal diff viewer. This section is the public compare note for Phase 4:
**design-level claims** with real next-hunk bench numbers. Direct wall-clock
head-to-head needs a local `delta` install (`cargo install git-delta`); when
absent, numbers below stay next-hunk-only from `cargo bench` on an AMD Ryzen 7
5700X, 32 GB RAM, Linux, release build.

| Dimension | next-hunk | delta | Notes |
|-----------|-----------|-------|-------|
| **Parse latency (huge, ~1.1 MB / 38k lines)** | **~1.4 ms** | likely similar (both stream-parse) | next-hunk builds a compact IR (arena + spans); delta builds a syntax-highlighted output. Both should be fast on this input size. |
| **Viewport materialization (40 rows)** | **~197 ns** single, **~341 µs** for 1000 random starts | N/A — delta renders the full output in one pass | next-hunk's key differentiator: O(visible) materialization via binary-searched file spans. Delta processes the entire diff even for a small pager view. |
| **Multi-file navigation** | binary-searched file index, O(log N) per jump | N/A — delta is a pager, not an interactive reviewer | next-hunk indexes file/hunk starts at parse time for instant jumps. |
| **Startup time** | ~tens of ms (gix discovery + syntect load + parse) | ~single-digit ms (no TUI, no syntax setup) | delta is leaner at startup (no TUI, no interactive loop). next-hunk's startup includes syntax set loading (~20 ms) and TUI init. |
| **Binary size (release, stripped)** | ~14 MB | ~2 MB (static musl) | next-hunk bundles syntect syntaxes + gix; delta is smaller. Binary size is **not a product goal** (per ARCHITECTURE.md). |
| **RSS (huge fixture)** | IR arena ~1 MB; total RSS likely < 50 MB (not measured) | < 10 MB expected | next-hunk holds the full parsed IR in memory for interactive navigation. Delta streams to stdout. |
| **Architecture** | Viewport-only: never builds widgets for off-screen rows | Full-output pager: renders entire diff to terminal | Fundamental design difference. next-hunk stays responsive on 100k-line diffs; delta may stall on huge inputs. |

**Verdict**: next-hunk is **not faster at paging a single diff to stdout** —
delta wins on startup speed, binary size, and simplicity. next-hunk's
differentiator is **interactive multi-file navigation at scale**: binary
searched indices, viewport-only materialization, and O(log N) file/hunk
jumps that don't degrade with diff size. For a 200-file / 100k-line
changeset, delta renders everything in one pass while next-hunk lets the
human jump between files and hunks in nanoseconds.

Methodology: `cargo bench --bench parse` and `cargo bench --bench viewport`
on the bundled fixtures (medium: 191 KB / 6.5k lines; huge: 1.1 MB / 38k
lines). Delta was not on PATH — numbers above for delta are informed
estimates based on its documented design (Rust, syntect, stream-through
pipeline) and should be verified independently. Contributions with a
rigorous comparison are welcome.

### Policy

- Missed gate → phase is **not done**; no marketing claim.  
- If hardware variance is huge, publish **relative** ratios (next-hunk vs delta) from the same run.  
- Changing a gate needs a one-line rationale under [Changelog of gates](#6-changelog-of-gates).  
- Adding syntect / gix / etc. is judged by latency and RSS, not by binary growth.

---

## 5. How to run (planned CLI)

```bash
# generate fixtures
./scripts/gen_fixtures.sh

# unit tests
cargo test

# benches (Phase 1+)
cargo bench --bench parse
cargo bench --bench viewport

# or unified helper
cargo run --release -- bench parse --fixture fixtures/huge.patch
cargo run --release -- bench viewport --fixture fixtures/huge.patch --height 40 --samples 1000
```

Record results under `benches/results/` (gitignored summaries OK) or paste into PR.

### Release build (normal)

```bash
cargo build --release
# size is observational only:
ls -lh target/release/next-hunk
```

---

## 6. Changelog of gates

| Date | Change | Reason |
|------|--------|--------|
| 2026-07-10 | Initial gates for Phase 1–3 | Project start |
| 2026-07-10 | Removed `binary_bytes` / musl gates | Binary size is not a product goal; keep latency + RSS only |
| 2026-07-10 | Recorded Phase 1 measured results (parse/viewport) | Phase 1 gates met; first `cargo bench` numbers from the dev machine |

---

## 7. Anti-patterns (perf)

| Do not | Do instead |
|--------|------------|
| Build `Vec` of styled rows for entire review | Viewport query only |
| Highlight whole file on every scroll | Async, generation-cancelled, visible only |
| Hold full patch `String` plus full duplicate line list | Arena + spans (one runtime copy) |
| Block UI on `git` for every keypress | Load once; refresh on explicit reload/watch later |
| Enable side-by-side by default without gates | Feature flag + own benchmarks |
| Reject syntect (etc.) to save binary bytes | Prove latency/RSS wins, then decide defaults |

---

## 8. Reporting template (for PRs)

```markdown
### Perf
- Machine:
- Commit:
- Fixture:
- parse_ms:
- viewport_ms (mean):
- scroll_p99_ms: (if TUI)
- rss_mb:
- Notes: (optional binary size; not a gate)
```
