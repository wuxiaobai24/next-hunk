# next-hunk

**Faster review of huge diffs. Built for agent-era workflows.**

**English** | [中文](./README_zh.md)

High-performance terminal review engine for large changesets.

| Pillar | Commitment |
|--------|------------|
| **Performance** | Viewport-only rendering, compact runtime IR, hard bench gates |
| **Scale** | Multi-file review streams without loading every row into widgets |
| **Experience** | Interactive multi-file navigation, readable layout, scriptable CLI |
| **Agent-era** | Structured export for humans *and* coding agents (roadmap) |

Binary size is **not** a product goal. We optimize latency and runtime memory on large diffs, not “smallest binary wins”.

## Name

**next-hunk** — navigate the review stream by file and hunk. CLI: `next-hunk`. Short alias later if we want (`nh`).

## Status

Early prototype (`v0.1.0-dev`):

- [x] Project scaffold + compact unified-diff IR (runtime model)
- [x] Viewport query skeleton
- [ ] Virtualized multi-file TUI
- [x] gix-backed worktree / staged / show (+ patch stdin)
- [ ] Async syntax highlight
- [ ] Agent export (JSON / Markdown)
- [ ] Public perf benchmarks vs common tools (e.g. delta; latency / RSS)

## Install (dev)

```bash
cargo install --path .
# or
cargo run --release -- diff
```

## Usage (planned)

```bash
next-hunk                  # working tree diff
next-hunk diff --staged
next-hunk show HEAD
git diff | next-hunk patch -
```

## Architecture (short)

```
git/patch ──► runtime IR (byte/line spans) ──► viewport query ──► TUI
```

Never build a full widget tree for every diff line. The IR is the source of truth; the UI materializes only what is on screen (+ small overscan).

**Full write-up:**

| Doc | Contents |
|-----|----------|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Position, layers, IR, phases, risks ([中文](docs/ARCHITECTURE_zh.md)) |
| [docs/PERF.md](docs/PERF.md) | Fixtures, metrics, phase gates, anti-patterns ([中文](docs/PERF_zh.md)) |

## License

MIT
