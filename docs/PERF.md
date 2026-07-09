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
