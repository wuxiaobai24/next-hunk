# MCP session control plane

next-hunk can expose its live **`serve`** session as a [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) server over **stdio**. Agents that speak MCP (Claude Code, Codex, OpenCode, Cursor, …) get first-class tools instead of multi-step shell invocations.

This is a **lightweight** mapping of the existing Unix-socket protocol — not a remote multi-tenant broker.

## Requirements

- Feature `mcp` (on by default; omit with cargo features if you want a leaner binary)
- Feature `serve` + a Unix OS for tool calls that touch a live session
- A human-owned `next-hunk serve` in the target worktree (MCP does not open a TUI)

## Start the server

```bash
next-hunk mcp
# Speaks JSON-RPC 2.0, newline-delimited, on stdin/stdout.
# Do not mix with interactive prompts on the same streams.
```

## Tools

| Tool | CLI equivalent | Purpose |
|------|----------------|---------|
| `list_sessions` | `next-hunk list` | Live sessions: hash, socket, repo root, file count |
| `review_structure` | `next-hunk review` | File/hunk JSON (no full patch text) |
| `navigate` | `next-hunk navigate` | Scroll human TUI to path / line / hunk |
| `add_comment` | `next-hunk comment add` | Line / range / hunk comment |
| `get_decision` | `next-hunk decision` | Accept/reject buckets JSON |
| `push_focus_note` | `next-hunk push` | Focus + `--note` grammar |
| `reload` | `next-hunk reload` | Refresh diff (`serve --watch`) |

Optional argument **`hash`** (16-char session id from `list_sessions`) selects among multi-worktree sessions. When omitted, the current worktree's socket is used (same as the CLI).

Errors are structured MCP tool results (`isError: true` + text), including the same “no server” / “path not in review” messages as the CLI.

## Typical agent loop

1. Human: `next-hunk serve --all` (TTY)
2. Agent (via MCP): `list_sessions` → pick hash if needed
3. Agent: `review_structure` → plan
4. Agent: `navigate` + `add_comment` / `push_focus_note`
5. Human reviews in the TUI (`a`/`r`, visual `v`+`c`, …)
6. Agent: `get_decision` (live) or, after quit, CLI `last-export` / export stdout

Skill docs and shell CLI remain supported; MCP is an additional entry point.

## Host configuration

### Claude Code

Add to project or user MCP settings (path/command may vary by install):

```json
{
  "mcpServers": {
    "next-hunk": {
      "command": "next-hunk",
      "args": ["mcp"]
    }
  }
}
```

If the binary is not on `PATH`:

```json
{
  "mcpServers": {
    "next-hunk": {
      "command": "/path/to/next-hunk",
      "args": ["mcp"]
    }
  }
}
```

### Generic MCP host (stdio)

Any host that can spawn a stdio MCP server:

```text
command: next-hunk
args:    ["mcp"]
cwd:     optional — tools that default to “current worktree” use this process cwd
```

Set `cwd` to the worktree you care about so default (no-`hash`) tools hit the right socket. For multi-worktree agents, always pass `hash` from `list_sessions`.

### Codex / OpenCode

Same pattern: register a stdio MCP server with command `next-hunk` and args `["mcp"]`. Consult the host’s MCP config docs for the exact JSON key names.

## Feature gate / lean builds

```bash
# Default build includes mcp
cargo build --release

# Explicit
cargo build --release --features mcp

# Omit MCP (and optionally other defaults)
cargo build --release --no-default-features --features "highlight,watch,serve"
```

No extra crates are pulled for MCP (uses existing `serde_json`). The feature exists so constrained builds can drop the surface and so future heavier transports can stay optional.

## Protocol notes

- Transport: **stdio**, one JSON-RPC message per line (no Content-Length framing)
- Protocol version: `2024-11-05`
- Methods: `initialize`, `ping`, `tools/list`, `tools/call`, empty `resources/list` / `prompts/list`
- Implementation lives in `src/mcp.rs`; session I/O in `src/session_client.rs` (shared with CLI intent)

## Non-goals

- HTTP / WebSocket session broker
- Replacing the skill document or CLI
- Windows named-pipe / TCP serve (deferred to 0.9; see [`PLATFORMS.md`](./PLATFORMS.md))
