# Platform support matrix

What next-hunk can do on each OS **today**, and what is explicitly deferred.
Source of truth for agents and humans; keep `src/platform.rs` error text and
this file aligned.

## Summary

| Capability | Linux | macOS | Windows |
|---|:---:|:---:|:---:|
| **Install** via `cargo install` / from source | ✅ | ✅ | ✅ |
| **Prebuilt** Release / `install.sh` | ✅ (musl x86_64/aarch64) | ✅ (arm64/x86_64) | ❌ (use cargo) |
| **Homebrew** | — | ✅ | — |
| `diff` / `show` / `filediff` interactive TUI | ✅ | ✅ | ✅ |
| `pager` (`git core.pager`) | ✅ | ✅ | ✅ |
| `inspect` / `inspect --json` (headless) | ✅ | ✅ | ✅ |
| `--select` + quit JSON / `last-export` | ✅ | ✅ | ✅ |
| `overlay` (tmux popup / zellij float) | ✅ | ✅ | ⚠️ in-place TTY only¹ |
| Live **`serve`** + session CLI (`push`, `decision`, `list`, `get`, `review`, `navigate`, `comment`, `reload`) | ✅ | ✅ | ❌ deferred → **0.9** |
| **MCP** live tools (`next-hunk mcp` → session tools) | ✅ | ✅ | ❌ same gate as serve |
| Auto-forward `diff --focus/--note` into live serve | ✅ | ✅ | ❌ (no live serve) |

¹ On Windows there is no first-class tmux/zellij path. `overlay` falls through to
an in-process one-shot review when stdout is a TTY; headless (no mux, no TTY)
errors with the same degradation message as on Unix.

## Windows: what works

**Supported now** (covered by CI `windows-latest` `cargo test --all-features`):

- One-shot review: `next-hunk diff`, `show`, `filediff`, `patch`
- Headless structure: `next-hunk inspect` / `inspect --json`
- Pager mode for git
- Approval gate: `diff --select` (blocks until quit; JSON on stdout)
- Post-quit recovery: `last-export` (cache under `.git/next-hunk/`)
- Config, themes, layouts (`unified` / `stack` / `split` / `auto`), highlight, word-diff

**Not supported until 0.9** (commands exist but exit non-zero with a clear
error pointing here):

- `serve`, `push`, `decision`, `list`, `get`, `review`, `navigate`, `comment`, `reload`
- MCP tools that need a live session
- `diff` auto-forward into a live serve (no socket to find)

### Windows agent playbook (until 0.9)

Prefer one-shot paths — do **not** open `serve`:

```bash
# Blocking approval + full report
next-hunk diff --all --include-untracked --select --export-on-quit json \
  --focus <path> --note banner="please review"

# Headless structure (no TUI)
next-hunk inspect --json

# After the human quit and you missed stdout
next-hunk last-export
```

Error text for session commands names the subcommand, points at this doc, and
suggests `--select` / `overlay` / `last-export` (see `platform::live_session_unavailable`).

## Why live serve is Unix-only today

`serve` binds a **Unix domain socket** keyed by worktree root
(`runtime_socket_path`). Clients (`push`, `decision`, MCP, auto-forward) connect
to that path. UDS is available on Linux/macOS; Windows has no drop-in equivalent
in the current stack (std-only, no extra crates for pipes yet).

This is an intentional product boundary (see `docs/PLAN.md`: full Windows serve
is **out of 0.8**, pushed to **0.9**), not a silent no-op.

## Roadmap: Windows serve (0.9)

When landed, keep the **same length-prefixed JSON frame** so CLI/MCP parsers
stay shared. Candidate transports (pick one, document here when implemented):

1. **Localhost TCP** loopback with a deterministic port or port file under
   `%LOCALAPPDATA%/next-hunk/`
2. **Named pipe** (`\\.\pipe\next-hunk-<hash>`) with the same frame codec

Non-goals for the first Windows serve cut:

- Remote multi-host broker
- Changing the agent-facing JSON shapes
- Breaking Linux/macOS UDS paths

## CI gate

`.github/workflows/ci.yml` runs `cargo test --all-features` on:

- `ubuntu-latest`
- `macos-latest`
- `windows-latest`

Windows CI is the gate that **pager / parse / inspect / one-shot paths stay
green**. Live-session integration tests are `cfg(all(unix, feature = "serve"))`
and do not run on Windows (by design).

## Related docs

- Product roadmap: [`PLAN.md`](./PLAN.md)
- MCP (Unix live tools): [`MCP.md`](./MCP.md)
- Agent skill: `skill/next-hunk/SKILL.md`
- Code: `src/platform.rs`, `src/tui/server.rs`, `src/session_client.rs`
