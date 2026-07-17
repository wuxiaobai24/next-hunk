# VCS backends: git and Jujutsu (jj)

next-hunk reviews **unified diffs**. The IR + viewport path is VCS-agnostic;
only the **source adapter** that produces the patch text differs.

| Backend | How | Config |
|---------|-----|--------|
| **git** | In-process [gix](https://github.com/GitoxideLabs/gitoxide) (no `git` CLI) | `vcs = "git"` |
| **jj** | Subprocess `jj … --git` (requires `jj` on `PATH`) | `vcs = "jj"` |
| **auto** (default) | Prefer **jj** when a `.jj` workspace is found (including colocated git+jj); else git | `vcs = "auto"` |

```toml
# .next-hunk/config.toml  or  ~/.config/next-hunk/config.toml
vcs = "auto"   # auto | git | jj
```

CLI override (where supported):

```bash
next-hunk diff --vcs jj
next-hunk show @- --vcs jj
next-hunk serve --vcs auto
next-hunk inspect --vcs git
```

## Detection

Walking ancestors of the cwd:

1. Nearest `.jj/` → Jujutsu workspace root  
2. Nearest `.git` (dir or file) → git worktree root  
3. **Colocated** (both at the same root): **auto → jj** so pure-jj workflows do not depend on the git index layer. Force git with `vcs = "git"`.

## Command mapping

| next-hunk | git (gix) | jj CLI |
|-----------|-----------|--------|
| `diff` (worktree) | index vs worktree | `jj diff --git` |
| `diff --staged` | HEAD vs index | **empty** (jj has no index); stderr note |
| `diff --all` | staged + unstaged | same as worktree |
| `diff --include-untracked` | adds untracked files | **ignored** (stderr note); new files usually already in the WC snapshot |
| `show <rev>` | commit vs parent | `jj diff --git -r <rev>` |
| `show A..B` | tree diff A→B | `jj diff --git --from A --to B` |
| `show A...B` | merge-base…B | `jj diff --git --from 'heads(::A & ::B)' --to B` |
| `filediff` | gix blob diff | system `diff -u` (no object store) |
| `serve` / skill | same adapters | same; socket keyed by workspace root |

All jj paths re-enter **`parse_unified_diff`** — the same compact IR as git and
`patch -`. Performance gates (parse + viewport benches) are unchanged.

## Behaviour differences vs git (and vs [hunk](https://github.com/modem-dev/hunk))

1. **No staging area** — `--staged` / `scope = "staged"` is a no-op under jj. Use
   plain `diff` for working-copy changes; use `show <rev>` for historical
   commits.
2. **Working-set** (`--all`) does not split `S`/`M` buckets under jj; rail marks
   are all `M` (modified) when origins are present.
3. **Untracked** — jj’s default snapshot tracks new files into `@`. If you use
   restrictive `snapshot.auto-track`, those paths may not appear in `jj diff`;
   next-hunk does not yet synthesize untracked file patches for jj.
4. **Rev syntax** — prefer jj revsets (`@`, `@-`, bookmarks). Git-style
   `A..B` / `A...B` is translated best-effort; exotic git ranges may need
   explicit `--from`/`--to` via shelling out to `jj` yourself and piping
   `next-hunk patch -`.
5. **Binary / submodule** — whatever `jj diff --git` emits is what you review;
   edge cases match jj, not gix.
6. **Watch / reload** — re-runs the same adapter (`jj diff` again). Snapshot
   cost is jj’s, not next-hunk’s IR rebuild policy.

Sapling (`sl`) is **not** implemented (optional later phase).

## Agent / skill notes

In a jj workspace, the usual agent recipe stays the same:

```bash
next-hunk diff --focus path:line --note path:line="why"
# or full local change surface (git: --all --include-untracked;
# jj: plain diff is usually enough):
next-hunk diff
next-hunk serve   # then list / review / navigate / comment / decision
```

If both git and jj markers exist and you need gix semantics (index, staged), set
`vcs = "git"` in project config.
