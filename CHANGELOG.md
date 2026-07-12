# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## Distribution

As of 0.1.0, **next-hunk is distributed via GitHub**:

```bash
cargo install --git https://github.com/wuxiaobai24/next-hunk
```

It is intentionally **not** published to crates.io yet (GitHub-only distribution
chosen for the first release). A crates.io publish may happen in a later release.

### Static binary

Tagged releases also publish a **fully static, all-features** x86_64 musl
binary (single ~2.6 MB xz file, no runtime deps — runs on Alpine/distroless/old
glibc). Built automatically by the `release` workflow on every `v*` tag. See
the README "Prebuilt static binary" section.

## [0.1.0] - 2026-07-12

First usable release — a terminal review engine for large changesets.

### Added
- **Virtualized multi-file TUI** (ratatui): only visible rows are materialized,
  so 1 MB+ diffs scroll in constant time.
- **Compact unified-diff IR** with a binary-searched file/hunk index; parse
  tolerates renames, binary placeholders, no-newline-at-eof, and CRLF.
- **Git backends** via `gix`: working-tree diff, `--staged`, `git show <rev>`,
  plus `patch -` / file input for review sources.
- **Syntax highlight** (syntect, pure-Rust) cached per file, viewport-only.
- **Navigation:** `]h`/`[h` next/prev hunk (wraps files), `Tab`/`h`/`l` file
  rail, `/` content search, `f` path filter, `g`/`G` top/bottom.
- **`o` open in editor** — jump to the focused line in `$EDITOR`.
- **Ignore-whitespace toggle** (`W`) — collapse whitespace-only changes.
- **Watch mode** (`--watch`) — live-reload on file change, preserving
  scroll/selection. (Now a default feature.)
- **Pager mode** (`next-hunk pager`) — drop-in `core.pager` for git so
  everyday `git diff`/`show`/`log -p` launch the TUI.
- **Diff stats** in the status bar (per-file + total `+ins/−del`).
- **Themes** dark/light/auto (`t` cycle; `auto` reads `$COLORFGBG`), via
  `config.toml`.
- **Line-number gutter** (`#`), **word-level inline diff** (`w`), and
  **unified/split layout** (`s`).
- `inspect` subcommand for headless IR summaries (scripting).
- Benchmarks for parse + viewport materialization.
- MIT license, CHANGELOG, and CI (fmt, clippy `-D warnings`, cross-OS tests,
  no-default-features build).

### Performance
Measured on the bundled fixtures (release build):

| bench | input | median |
|---|---|---|
| `parse/huge` | 1.1 MB / 38k-line diff | **~1.3 ms** |
| `parse/medium` | 191 KB / 6.5k-line diff | ~150 µs |
| `parse/small` | 6 KB / 213-line diff | ~5 µs |
| `viewport_huge_h40` | materialize a 40-row window over the huge diff | **~300 µs** |
| `viewport_single_h40` | resolve a file span + clip to viewport | **~190 ns** |

Parsing a megabyte-scale diff in single-digit milliseconds and rendering a
viewport in sub-microsecond to sub-millisecond range is what makes the tool
stay responsive on changesets that stall other viewers.

[Unreleased]: https://github.com/wuxiaobai24/next-hunk/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.1.0
