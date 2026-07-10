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
- [x] Viewport query with binary search on file spans
- [x] Virtualized multi-file TUI (ratatui)
- [x] gix-backed worktree / staged / show (+ patch stdin)
- [x] Robust unified parse (rename, binary placeholder, no-newline, CRLF)
- [x] Benchmarks: parse + viewport materialization
- [x] Syntax highlight (syntect, viewport-only + cached, default on)
- [x] Search: in-stream `/` content search + file-rail `f` path filter
- [x] Hunk navigation: `]h` / `[h` next/prev hunk (binary-searched hunk index)
- [x] Watch mode: `--watch` live-reload (notify, debounce; preserves scroll/selection)
- [ ] Async syntax highlight (gen-id cancellation; current impl is sync viewport-only)
- [ ] Agent export (JSON / Markdown)
- [ ] Public perf benchmarks vs common tools (e.g. delta; latency / RSS)

## Install (dev)

```bash
cargo install --path .
# or
cargo run --release -- diff
# with live-reload watch mode (optional feature):
cargo run --release --features watch -- diff --watch
```

## Usage

```bash
next-hunk                  # working tree diff
next-hunk diff --staged
next-hunk diff --watch     # live-reload on file changes (needs `watch` feature)
next-hunk show HEAD
git diff | next-hunk patch -
next-hunk inspect path/to.patch   # IR summary, no TUI (scripting)
```

### Keybindings

| Key | Action |
|-----|--------|
| `j` / `↓` | scroll down one row |
| `k` / `↑` | scroll up one row |
| `J` / `PgDn` | scroll half a page |
| `K` / `PgUp` | scroll half a page up |
| `g` / `Home` | jump to top |
| `G` / `End` | jump to bottom |
| `]h` | next hunk (wraps across files) |
| `[h` | previous hunk (wraps across files) |
| `Tab` / `l` / `→` | next file |
| `Shift+Tab` / `h` / `←` | previous file |
| `H` | toggle syntax highlight |
| `/` | search diff content (then `n`/`N` next/prev) |
| `f` | filter file rail by path substring |
| `q` / `Esc` / `Ctrl+C` | quit (`Esc` clears active search first) |

## Configuration

Persist preferences in a `config.toml` instead of re-typing flags. Two layers,
merged with CLI flags (highest precedence first):

```text
CLI flag  >  .next-hunk/config.toml (project)  >  ~/.config/next-hunk/config.toml (user)  >  defaults
```

Fields:

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `staged` | bool | `false` | review staged changes |
| `highlight` | bool | `true` | syntax highlighting |
| `watch` | bool | `false` | live-reload on file changes (needs `watch` feature) |
| `line_numbers` | bool | — | _P1: accepted, not yet rendered_ |
| `wrap_lines` | bool | — | _P1: accepted, not yet rendered_ |
| `theme` | string | — | _P1: accepted, not yet applied_ |

Example `~/.config/next-hunk/config.toml`:

```toml
highlight = true
watch = true
```

CLI overrides: `--staged`, `--watch`, `--no-highlight`.

## Testing & benchmarks

```bash
# unit + integration + headless TUI tests
cargo test

# generate fixtures (small / medium / huge)
./scripts/gen_fixtures.sh

# benchmarks (PERF.md metrics)
cargo bench --bench parse
cargo bench --bench viewport
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
