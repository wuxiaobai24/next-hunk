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

`v0.1.0-dev` — usable daily driver for reviewing diffs:

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
- [x] Pager mode: `next-hunk pager` as git's `core.pager`
- [x] Open in editor: `o` jumps to the focused line in `$EDITOR`
- [x] Diff stats in the status bar (per-file + total `+ins/−del`)
- [x] Ignore-whitespace toggle (`W`, collapses whitespace-only changes)
- [ ] Async syntax highlight (gen-id cancellation; current impl is sync viewport-only)
- [ ] Agent export (JSON / Markdown)
- [ ] Public perf benchmarks vs common tools (e.g. delta; latency / RSS)

## Performance

Parsing and rendering stay in the millisecond / sub-millisecond range even on
megabyte-scale diffs, because the runtime IR is compact and only visible rows
are ever materialized. Measured on the bundled fixtures (release build, one
core; reproduce with `cargo bench`):

| benchmark | input | median |
|---|---|---|
| `parse/huge` | 1.1 MB / 38k-line diff | **~1.3 ms** |
| `parse/medium` | 191 KB / 6.5k-line diff | ~150 µs |
| `parse/small` | 6 KB / 213-line diff | ~5 µs |
| `viewport_huge_h40` | materialize a 40-row window over the huge diff | **~300 µs** |
| `viewport_single_h40` | resolve a file span + clip to viewport | **~190 ns** |

The key idea: navigation (`]h`/`[`h`, file rail) resolves against a
binary-searched index in **nanoseconds**, independent of total diff size, and
the viewport materializes only what's on screen. Full numbers live in
[CHANGELOG.md](./CHANGELOG.md).

## Install

```bash
# from crates.io (once published)
cargo install next-hunk
# or build from source
cargo install --path .
# or just run it
cargo run --release -- diff
```

### Set as git's pager (recommended)

Once installed, make everyday `git diff` / `show` / `log` open the review TUI:

```bash
git config --global core.pager "next-hunk pager"
```

## Usage

```bash
next-hunk                  # working tree diff
next-hunk diff --staged
next-hunk diff --watch     # live-reload on file changes
next-hunk show HEAD
git diff | next-hunk patch -
next-hunk inspect path/to.patch   # IR summary, no TUI (scripting)

# Use next-hunk as git's pager for everyday diff/show/log:
git config core.pager "next-hunk pager"
git diff        # → launches the review TUI
git show HEAD   # → launches the review TUI
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
| `#` | toggle line-number gutter |
| `w` | toggle word-level inline diff |
| `W` | toggle ignore-whitespace (hide whitespace-only changes) |
| `s` | toggle unified / split layout |
| `t` | cycle theme: dark → light → auto |
| `/` | search diff content (then `n`/`N` next/prev) |
| `f` | filter file rail by path substring |
| `o` | open the focused line in `$EDITOR` (at that line) |
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
| `watch` | bool | `false` | live-reload on file changes |
| `line_numbers` | bool | — | show old/new line-number gutter (`#` toggles) |
| `wrap_lines` | bool | — | _not yet rendered_ |
| `theme` | string | `"dark"` | `"dark"` / `"light"` / `"auto"` (`t` cycles) |

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
