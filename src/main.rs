//! next-hunk CLI entry.

use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use next_hunk::config::{CliFlags, Config, LayoutMode, ResolvedConfig};
use next_hunk::ir::{parse_unified_diff, Review};
use next_hunk::source::{find_repo, git_diff, git_file_diff, git_show, open_repo};
use next_hunk::tui::{run_review_tui, ReviewOptions};

#[derive(Debug, Parser)]
#[command(
    name = "next-hunk",
    version,
    about = "High-performance terminal review engine for large changesets"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Review working-tree (or staged) diff.
    Diff {
        /// Review staged changes (`git diff --cached`).
        #[arg(long, short = 's')]
        staged: bool,
        /// Re-run on filesystem changes (live reload). Requires the `watch`
        /// feature; otherwise reports that it is unavailable.
        #[arg(long)]
        watch: bool,
        /// Disable syntax highlighting (overrides config/highlight default).
        #[arg(long)]
        no_highlight: bool,
        /// Include untracked files in worktree diff (default: off).
        #[arg(long)]
        include_untracked: bool,
        /// Scroll to this location on startup: `<path>` / `<path>:<line>` /
        /// `<path>:h<n>` (1-based hunk ordinal). Agent-bridge: point the human
        /// at what matters.
        #[arg(long)]
        focus: Option<String>,
        /// Attach an agent annotation, repeatable: `<path>:<line>=<text>` /
        /// `<path>:h<n>=<text>` / `banner=<text>`. Shown in the TUI to explain
        /// the change to the human.
        #[arg(long, action = clap::ArgAction::Append)]
        note: Vec<String>,
        /// Selection gate: the human accepts/rejects each hunk (`a`/`r`/`u`),
        /// and on quit the decisions are emitted as JSON on stdout for the
        /// agent to parse. Requires an interactive terminal.
        #[arg(long)]
        select: bool,
        /// Optional pathspecs to limit the review.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        extra: Vec<String>,
    },
    /// Review a commit or range (`git show` / `git diff A..B`).
    Show {
        /// Revision or range (e.g. HEAD, main..HEAD).
        rev: String,
    },
    /// Diff two arbitrary files on disk.
    Filediff {
        /// First file (old).
        old: PathBuf,
        /// Second file (new).
        new: PathBuf,
    },
    /// Review a unified patch from a file or stdin (`-`).
    Patch {
        /// Path to patch file, or `-` for stdin.
        path: PathBuf,
    },
    /// Git pager mode. Reads a unified diff from stdin and opens the TUI.
    ///
    /// Designed for `git config core.pager "next-hunk pager"` so that everyday
    /// `git diff` / `git show` / `git log -p` launch the review TUI directly.
    /// Behaves like `patch -`, but an empty stdin is a clean no-op (exit 0)
    /// rather than an error, because git frequently pipes nothing to its pager
    /// (e.g. `git diff` with no changes).
    Pager,
    /// Print IR summary without opening the TUI (engine smoke / scripting).
    Inspect {
        /// Path to patch file, or `-` for stdin. If omitted, uses worktree diff.
        path: Option<PathBuf>,
        #[arg(long, short = 's')]
        staged: bool,
    },
    /// Open a persistent review TUI that also listens for agent pushes.
    ///
    /// Like `diff`, but the TUI stays open and a separate `next-hunk push` /
    /// `next-hunk decision` process can stream updates into it (focus/notes)
    /// and read the human's accumulated decisions in real time. The TUI runs
    /// with selection mode on (a/r/u per hunk), so `decision` returns real
    /// accept/reject results.
    Serve {
        /// Review staged changes (`git diff --cached`).
        #[arg(long, short = 's')]
        staged: bool,
        /// Re-run on filesystem changes (live reload). Requires the `watch`
        /// feature.
        #[arg(long)]
        watch: bool,
        /// Disable syntax highlighting (overrides config/highlight default).
        #[arg(long)]
        no_highlight: bool,
        /// Include untracked files in worktree diff (default: off).
        #[arg(long)]
        include_untracked: bool,
        /// Scroll to this location on startup: `<path>` / `<path>:<line>` /
        /// `<path>:h<n>` (1-based hunk ordinal).
        #[arg(long)]
        focus: Option<String>,
        /// Attach an agent annotation, repeatable: `<path>:<line>=<text>` /
        /// `<path>:h<n>=<text>` / `banner=<text>`.
        #[arg(long, action = clap::ArgAction::Append)]
        note: Vec<String>,
        /// Optional pathspecs to limit the review.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        extra: Vec<String>,
    },
    /// Push a focus/note update into a running `next-hunk serve` in this repo.
    ///
    /// Requires that `next-hunk serve` is already running (the server owns the
    /// TUI). The pushed focus/notes appear live in that TUI; this command
    /// returns immediately with `ok` (or an error message).
    Push {
        /// Scroll the running TUI to this location: `<path>` / `<path>:<line>` /
        /// `<path>:h<n>`.
        #[arg(long)]
        focus: Option<String>,
        /// Attach an agent annotation into the running TUI, repeatable.
        #[arg(long, action = clap::ArgAction::Append)]
        note: Vec<String>,
    },
    /// Read the human's accumulated per-hunk decisions from a running `serve`.
    ///
    /// Prints one JSON line on stdout (same shape as `--select` quit output):
    /// `{"accepted":[...],"rejected":[...],"undecided":[...]}`. Returns
    /// immediately — does not wait for the human to quit. Requires a running
    /// `next-hunk serve` in this repo.
    Decision,
    /// List live next-hunk server sessions.
    ///
    /// Scans well-known socket directories for live servers. Prints one line
    /// per live session: `<repo-hash>  <socket-path>`.
    List,
    /// Show info about a running server session.
    ///
    /// Without an argument, checks the current repo's socket. With a hash
    /// argument, looks up a specific session by its repo hash.
    Get {
        /// Optional repo hash to look up (defaults to current repo).
        hash: Option<String>,
    },
    /// Print the current review's file/hunk structure as JSON.
    ///
    /// Connects to a running serve session and dumps the file/hunk summary
    /// (paths, insert/delete counts, hunk ranges) without full patch text.
    /// Without an argument, uses the current repo's socket.
    Review {
        /// Optional repo hash to look up (defaults to current repo).
        hash: Option<String>,
    },
    /// Navigate a running serve session to a file, hunk, or line.
    ///
    /// Uses the same `--focus` syntax: `<path>`, `<path>:<line>`, or
    /// `<path>:h<n>` (1-based hunk ordinal).
    Navigate {
        /// Navigation target: `<path>` / `<path>:<line>` / `<path>:h<n>`.
        target: String,
        /// Optional repo hash to look up (defaults to current repo).
        #[arg(long)]
        hash: Option<String>,
    },
    /// Manage comments on a running serve session.
    Comment {
        #[command(subcommand)]
        action: CommentAction,
    },
    /// Reload the running serve session's diff content.
    ///
    /// Re-fetches the diff from the same source the serve was started with
    /// and refreshes the review, preserving focus/notes/decisions best-effort.
    /// Requires the serve to have been started with `--watch` (or a reloader).
    Reload {
        /// Optional repo hash to look up (defaults to current repo).
        #[arg(long)]
        hash: Option<String>,
    },
}

#[derive(Debug, clap::Subcommand)]
enum CommentAction {
    /// Add a comment to a file.
    Add {
        /// Target file path.
        #[arg(long)]
        file: String,
        /// Optional new-side source line number.
        #[arg(long)]
        line: Option<u32>,
        /// Optional hunk ordinal (1-based).
        #[arg(long)]
        hunk: Option<usize>,
        /// Optional repo hash.
        #[arg(long)]
        hash: Option<String>,
        /// Comment text.
        text: String,
    },
    /// List all comments.
    List {
        /// Optional repo hash.
        #[arg(long)]
        hash: Option<String>,
    },
    /// Remove a comment by id.
    Rm {
        /// Comment id to remove.
        id: String,
        /// Optional repo hash.
        #[arg(long)]
        hash: Option<String>,
    },
    /// Apply comments as TUI notes.
    Apply {
        /// Optional repo hash.
        #[arg(long)]
        hash: Option<String>,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Commands::Diff {
        staged: false,
        watch: false,
        no_highlight: false,
        include_untracked: false,
        focus: None,
        note: Vec::new(),
        select: false,
        extra: Vec::new(),
    }) {
        Commands::Diff {
            staged,
            watch,
            no_highlight,
            include_untracked,
            focus,
            note,
            select,
            extra,
        } => {
            // Parse the agent-bridge specs and check the --select tty
            // requirement BEFORE touching the repo, so a bad spec or a
            // non-interactive --select fails fast with a clear message (an
            // agent scripting this gets actionable feedback, not a git error).
            let focus_target = focus
                .map(|s| next_hunk::cli_parse::parse_focus(&s))
                .transpose()?;
            let notes = note
                .iter()
                .map(|s| next_hunk::cli_parse::parse_note(s))
                .collect::<Result<Vec<_>>>()?;

            // `--select` is a blocking interactive gate; it can't run without
            // a real terminal.
            if select && !std::io::stdout().is_terminal() {
                bail!("--select requires an interactive terminal (stdout is not a tty)");
            }

            let cwd = std::env::current_dir()?;
            let repo = find_repo(&cwd)?;

            // Layered config: project (.next-hunk/config.toml) > user
            // (~/.config/next-hunk/config.toml). CLI flags override on top.
            let cfg = Config::load(&cwd);
            let resolved = ResolvedConfig::resolve(
                &cfg,
                &CliFlags {
                    staged: if staged { Some(true) } else { None },
                    watch: if watch { Some(true) } else { None },
                    highlight: if no_highlight { Some(false) } else { None },
                    include_untracked: if include_untracked { Some(true) } else { None },
                },
            );

            if resolved.watch && !next_hunk::tui::watch::Watcher::is_enabled() {
                eprintln!(
                    "note: `--watch` requires the `watch` feature (rebuild with --features watch)"
                );
            }

            let text = git_diff(&repo, resolved.staged, &extra, resolved.include_untracked)?;
            let reloader = if resolved.watch {
                Some(make_diff_reloader(
                    repo.clone(),
                    resolved.staged,
                    extra,
                    resolved.include_untracked,
                ))
            } else {
                None
            };
            open_review_from_text(
                &text,
                reloader,
                resolved.highlight,
                resolved.line_numbers,
                resolved.wrap,
                resolved.theme,
                resolved.layout,
                Some(repo),
                ReviewOptions {
                    focus: focus_target,
                    notes,
                    select_mode: select,
                },
                None,
            )
        }
        Commands::Show { rev } => {
            let cwd = std::env::current_dir()?;
            let repo = find_repo(&cwd)?;
            let text = git_show(&repo, &rev)?;
            // `show` is a one-shot snapshot: no watch, highlight default on.
            // Honor the user/project theme config even for `show`.
            let cfg = Config::load(&cwd);
            open_review_from_text(
                &text,
                None,
                true,
                cfg.line_numbers.unwrap_or(true),
                cfg.wrap.unwrap_or(false),
                cfg.theme,
                LayoutMode::Unified,
                Some(repo),
                ReviewOptions::default(),
                None,
            )
        }
        Commands::Patch { path } => {
            let text = read_patch_input(&path)?;
            open_review_from_text(
                &text,
                None,
                true,
                true,
                false,
                None,
                LayoutMode::Unified,
                None,
                ReviewOptions::default(),
                None,
            )
        }
        Commands::Filediff { old, new } => {
            let cwd = std::env::current_dir()?;
            let repo = open_repo(&cwd)?;
            let text = git_file_diff(&repo, &old, &new)?;
            if text.trim().is_empty() {
                eprintln!("(files are identical)");
                return Ok(());
            }
            open_review_from_text(
                &text,
                None,
                true,
                true,
                false,
                None,
                LayoutMode::Unified,
                repo.workdir().map(|p| p.to_owned()),
                ReviewOptions::default(),
                None,
            )
        }
        Commands::Inspect { path, staged } => {
            let text = if let Some(path) = path {
                read_patch_input(&path)?
            } else {
                let repo = find_repo(&std::env::current_dir()?)?;
                git_diff(&repo, staged, &[], false)?
            };
            if text.trim().is_empty() {
                println!("files=0 stream_rows=0 arena_bytes=0");
                return Ok(());
            }
            let review = parse_review(&text)?;
            print_inspect(&review);
            Ok(())
        }
        Commands::Pager => {
            // Git pipes the diff to our stdin. Honor the user/project theme.
            let cwd = std::env::current_dir()?;
            let cfg = Config::load(&cwd);
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .context("read patch from stdin (pager mode)")?;
            // Empty stdin is a clean no-op in pager mode: git frequently invokes
            // its pager with nothing (e.g. `git diff` with no changes). Exit 0
            // so git sees a successful pager run.
            if buf.trim().is_empty() {
                return Ok(());
            }
            // `o` (open in editor) resolves relative paths against the repo
            // workdir if we're in one, else the cwd.
            let workdir = find_repo(&cwd).ok();
            open_review_from_text(
                &buf,
                None,
                true,
                cfg.line_numbers.unwrap_or(true),
                cfg.wrap.unwrap_or(false),
                cfg.theme,
                LayoutMode::Unified,
                workdir,
                ReviewOptions::default(),
                None,
            )
        }
        Commands::Serve {
            staged,
            watch,
            no_highlight,
            include_untracked,
            focus,
            note,
            extra,
        } => run_serve(
            staged,
            watch,
            no_highlight,
            include_untracked,
            focus,
            note,
            extra,
        ),
        Commands::Push { focus, note } => run_push(focus, note),
        Commands::Decision => run_decision(),
        Commands::List => run_list(),
        Commands::Get { hash } => run_get(hash),
        Commands::Review { hash } => run_review(hash),
        Commands::Navigate { target, hash } => run_navigate(target, hash),
        Commands::Comment { action } => run_comment(action),
        Commands::Reload { hash } => run_reload(hash),
    }
}

/// Build the live-reload closure for `--watch`: re-runs the same git diff.
/// Captures the repo path, staged flag, pathspecs, and untracked flag by value.
fn make_diff_reloader(
    repo: PathBuf,
    staged: bool,
    extra: Vec<String>,
    include_untracked: bool,
) -> next_hunk::tui::Reloader {
    Box::new(move || {
        git_diff(&repo, staged, &extra, include_untracked).context("re-run git diff for --watch")
    })
}

/// `next-hunk serve`: a persistent TUI that also accepts pushes via a Unix
/// socket. Mirrors the `diff` path (config layering, focus/note parsing,
/// optional `--watch`) but unconditionally enables select mode (the whole
/// point of `serve` is to collect decisions via `next-hunk decision`) and
/// binds a server listener on the repo's runtime socket path.
#[cfg(all(feature = "serve", unix))]
fn run_serve(
    staged: bool,
    watch: bool,
    no_highlight: bool,
    include_untracked: bool,
    focus: Option<String>,
    note: Vec<String>,
    extra: Vec<String>,
) -> Result<()> {
    // serve is interactive (it owns a TUI); require a real terminal up front.
    if !std::io::stdout().is_terminal() {
        bail!("serve requires an interactive terminal (stdout is not a tty)");
    }

    let focus_target = focus
        .map(|s| next_hunk::cli_parse::parse_focus(&s))
        .transpose()?;
    let notes = note
        .iter()
        .map(|s| next_hunk::cli_parse::parse_note(s))
        .collect::<Result<Vec<_>>>()?;

    let cwd = std::env::current_dir()?;
    let repo = find_repo(&cwd)?;

    let cfg = Config::load(&cwd);
    let resolved = ResolvedConfig::resolve(
        &cfg,
        &CliFlags {
            staged: if staged { Some(true) } else { None },
            watch: if watch { Some(true) } else { None },
            highlight: if no_highlight { Some(false) } else { None },
            include_untracked: if include_untracked { Some(true) } else { None },
        },
    );

    if resolved.watch && !next_hunk::tui::watch::Watcher::is_enabled() {
        eprintln!("note: `--watch` requires the `watch` feature (rebuild with --features watch)");
    }

    // Bind the server socket before opening the TUI, so a `push`/`decision`
    // issued the instant the TUI appears finds a live socket. A bind failure
    // (e.g. another serve running) is fatal and leaves no half-open TUI.
    let server = spawn_serve_listener(&repo)?;

    let text = git_diff(&repo, resolved.staged, &extra, resolved.include_untracked)?;
    let reloader = if resolved.watch {
        Some(make_diff_reloader(
            repo.clone(),
            resolved.staged,
            extra,
            resolved.include_untracked,
        ))
    } else {
        None
    };
    open_review_from_text(
        &text,
        reloader,
        resolved.highlight,
        resolved.line_numbers,
        resolved.wrap,
        resolved.theme,
        resolved.layout,
        Some(repo),
        ReviewOptions {
            focus: focus_target,
            notes,
            // serve exists to collect decisions, so select mode is always on.
            select_mode: true,
        },
        Some(server),
    )
}

/// `next-hunk push`: send a focus/note update to the running server in this
/// repo. Returns immediately with a short status line.
#[cfg(all(feature = "serve", unix))]
fn run_push(focus: Option<String>, note: Vec<String>) -> Result<()> {
    let focus_target = focus
        .map(|s| next_hunk::cli_parse::parse_focus(&s))
        .transpose()?;
    let notes = note
        .iter()
        .map(|s| next_hunk::cli_parse::parse_note(s))
        .collect::<Result<Vec<_>>>()?;

    let cwd = std::env::current_dir()?;
    let repo = find_repo(&cwd)?;
    let socket = next_hunk::cli_parse::runtime_socket_path(&repo);

    let command = next_hunk::tui::server::ServerCommand::Push {
        focus: focus_target,
        notes,
    };
    match next_hunk::tui::server::send_command(&socket, &command) {
        Ok(next_hunk::tui::server::ServerReply::Ok) => {
            println!("ok: pushed to running server");
            Ok(())
        }
        Ok(other) => {
            // The server replied with something other than Ok — surface it.
            bail!("unexpected server reply: {other:?}");
        }
        Err(e) => bail_on_no_server(e),
    }
}

/// `next-hunk decision`: read the human's accumulated decisions from the
/// running server, printed as one JSON line on stdout (same shape as
/// `--select` quit output, so an agent parses it identically).
#[cfg(all(feature = "serve", unix))]
fn run_decision() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo = find_repo(&cwd)?;
    let socket = next_hunk::cli_parse::runtime_socket_path(&repo);

    match next_hunk::tui::server::send_command(
        &socket,
        &next_hunk::tui::server::ServerCommand::Decision,
    ) {
        Ok(next_hunk::tui::server::ServerReply::Decisions(selections)) => {
            println!("{}", serde_json::to_string(&selections)?);
            Ok(())
        }
        Ok(next_hunk::tui::server::ServerReply::Error(msg)) => bail!("server error: {msg}"),
        Ok(other) => bail!("unexpected server reply: {other:?}"),
        Err(e) => bail_on_no_server(e),
    }
}

/// `next-hunk list`: discover live server sessions.
#[cfg(all(feature = "serve", unix))]
fn run_list() -> Result<()> {
    let sessions = next_hunk::cli_parse::discover_live_sockets();
    if sessions.is_empty() {
        println!("no live sessions found");
        return Ok(());
    }
    for (path, hash) in &sessions {
        // Try to get session info for richer output.
        let info = next_hunk::tui::server::send_command(
            path,
            &next_hunk::tui::server::ServerCommand::Info,
        );
        match info {
            Ok(next_hunk::tui::server::ServerReply::Info {
                repo_path,
                file_count,
            }) => {
                println!(
                    "{}  {}  files={file_count}  repo={repo_path}",
                    hash,
                    path.display()
                );
            }
            _ => {
                println!("{}  {}", hash, path.display());
            }
        }
    }
    Ok(())
}

/// `next-hunk get [hash]`: show info for a specific session.
#[cfg(all(feature = "serve", unix))]
fn run_get(hash: Option<String>) -> Result<()> {
    let socket = resolve_socket(hash)?;
    match next_hunk::tui::server::send_command(
        &socket,
        &next_hunk::tui::server::ServerCommand::Info,
    ) {
        Ok(next_hunk::tui::server::ServerReply::Info {
            repo_path,
            file_count,
        }) => {
            println!("socket: {}", socket.display());
            println!("repo:   {repo_path}");
            println!("files:  {file_count}");
            Ok(())
        }
        Ok(next_hunk::tui::server::ServerReply::Error(msg)) => bail!("server error: {msg}"),
        Ok(other) => bail!("unexpected server reply: {other:?}"),
        Err(e) => bail_on_no_server(e),
    }
}

/// `next-hunk review [hash]`: print the review structure as JSON.
#[cfg(all(feature = "serve", unix))]
fn run_review(hash: Option<String>) -> Result<()> {
    let socket = resolve_socket(hash)?;
    match next_hunk::tui::server::send_command(
        &socket,
        &next_hunk::tui::server::ServerCommand::Review,
    ) {
        Ok(next_hunk::tui::server::ServerReply::Review(summary)) => {
            println!("{}", serde_json::to_string_pretty(&summary)?);
            Ok(())
        }
        Ok(next_hunk::tui::server::ServerReply::Error(msg)) => bail!("server error: {msg}"),
        Ok(other) => bail!("unexpected server reply: {other:?}"),
        Err(e) => bail_on_no_server(e),
    }
}

/// `next-hunk comment <action>`: manage session comments.
#[cfg(all(feature = "serve", unix))]
fn run_comment(action: CommentAction) -> Result<()> {
    use next_hunk::tui::server::ServerCommand;
    match action {
        CommentAction::Add {
            file,
            line,
            hunk,
            hash,
            text,
        } => {
            let socket = resolve_socket(hash)?;
            match next_hunk::tui::server::send_command(
                &socket,
                &ServerCommand::CommentAdd {
                    file,
                    text,
                    line,
                    hunk,
                },
            ) {
                Ok(next_hunk::tui::server::ServerReply::CommentAdded { id }) => {
                    println!("ok: comment added with id {id}");
                    Ok(())
                }
                Ok(next_hunk::tui::server::ServerReply::Error(msg)) => {
                    bail!("server error: {msg}")
                }
                Ok(other) => bail!("unexpected server reply: {other:?}"),
                Err(e) => bail_on_no_server(e),
            }
        }
        CommentAction::List { hash } => {
            let socket = resolve_socket(hash)?;
            match next_hunk::tui::server::send_command(&socket, &ServerCommand::CommentList) {
                Ok(next_hunk::tui::server::ServerReply::CommentList { comments }) => {
                    if comments.is_empty() {
                        println!("no comments");
                    } else {
                        for c in &comments {
                            let loc = match (c.hunk, c.line) {
                                (Some(h), _) => format!(" hunk={h}"),
                                (_, Some(l)) => format!(" line={l}"),
                                _ => String::new(),
                            };
                            println!("{}  {}{}  {}", c.id, c.file, loc, c.text);
                        }
                    }
                    Ok(())
                }
                Ok(next_hunk::tui::server::ServerReply::Error(msg)) => {
                    bail!("server error: {msg}")
                }
                Ok(other) => bail!("unexpected server reply: {other:?}"),
                Err(e) => bail_on_no_server(e),
            }
        }
        CommentAction::Rm { id, hash } => {
            let socket = resolve_socket(hash)?;
            match next_hunk::tui::server::send_command(&socket, &ServerCommand::CommentRm { id }) {
                Ok(next_hunk::tui::server::ServerReply::Ok) => {
                    println!("ok: comment removed");
                    Ok(())
                }
                Ok(next_hunk::tui::server::ServerReply::Error(msg)) => {
                    bail!("server error: {msg}")
                }
                Ok(other) => bail!("unexpected server reply: {other:?}"),
                Err(e) => bail_on_no_server(e),
            }
        }
        CommentAction::Apply { hash } => {
            let socket = resolve_socket(hash)?;
            match next_hunk::tui::server::send_command(&socket, &ServerCommand::CommentApply) {
                Ok(next_hunk::tui::server::ServerReply::Ok) => {
                    println!("ok: comments applied to TUI");
                    Ok(())
                }
                Ok(next_hunk::tui::server::ServerReply::Error(msg)) => {
                    bail!("server error: {msg}")
                }
                Ok(other) => bail!("unexpected server reply: {other:?}"),
                Err(e) => bail_on_no_server(e),
            }
        }
    }
}

/// Resolve a socket path from an optional hash or the current repo.
#[cfg(all(feature = "serve", unix))]
fn resolve_socket(hash: Option<String>) -> Result<PathBuf> {
    if let Some(h) = &hash {
        let sessions = next_hunk::cli_parse::discover_live_sockets();
        let found = sessions.iter().find(|(_, hh)| hh == h);
        match found {
            Some((path, _)) => Ok(path.clone()),
            None => bail!("no live session with hash {h}"),
        }
    } else {
        let cwd = std::env::current_dir()?;
        let repo = find_repo(&cwd)?;
        Ok(next_hunk::cli_parse::runtime_socket_path(&repo))
    }
}

/// `next-hunk navigate <target> [--hash <hash>]`: navigate a serve TUI.
#[cfg(all(feature = "serve", unix))]
fn run_navigate(target: String, hash: Option<String>) -> Result<()> {
    let focus_target = next_hunk::cli_parse::parse_focus(&target)?;
    let socket = resolve_socket(hash)?;
    match next_hunk::tui::server::send_command(
        &socket,
        &next_hunk::tui::server::ServerCommand::Navigate {
            target: focus_target,
        },
    ) {
        Ok(next_hunk::tui::server::ServerReply::Ok) => {
            println!("ok: navigated to {}", target);
            Ok(())
        }
        Ok(next_hunk::tui::server::ServerReply::Error(msg)) => bail!("server error: {msg}"),
        Ok(other) => bail!("unexpected server reply: {other:?}"),
        Err(e) => bail_on_no_server(e),
    }
}

/// `next-hunk reload [--hash <hash>]`: reload the serve session's diff.
#[cfg(all(feature = "serve", unix))]
fn run_reload(hash: Option<String>) -> Result<()> {
    let socket = resolve_socket(hash)?;
    match next_hunk::tui::server::send_command(
        &socket,
        &next_hunk::tui::server::ServerCommand::Reload,
    ) {
        Ok(next_hunk::tui::server::ServerReply::Ok) => {
            println!("ok: session reloaded");
            Ok(())
        }
        Ok(next_hunk::tui::server::ServerReply::Error(msg)) => bail!("server error: {msg}"),
        Ok(other) => bail!("unexpected server reply: {other:?}"),
        Err(e) => bail_on_no_server(e),
    }
}

/// Turn a socket-connect failure into an actionable "no server" message,
/// while letting unrelated errors (e.g. malformed reply) pass through. Takes
/// the error by value so the unrelated-error path can return it as-is.
#[cfg(all(feature = "serve", unix))]
fn bail_on_no_server(err: anyhow::Error) -> Result<()> {
    // send_command wraps the connect step with "connect to server socket …".
    // A missing socket surfaces as a NotFound/WouldBlock/ConnectionRefused
    // underneath; we match on the textual context to stay decoupled from the
    // exact io::ErrorKind across platforms.
    let msg = format!("{err:#}");
    if msg.contains("connect to server socket") {
        bail!("no next-hunk server running in this repo; start one with `next-hunk serve`");
    }
    Err(err)
}

// --- serve-feature plumbing ------------------------------------------------
// On builds with the `serve` feature (default), bind the repo's runtime socket
// and wire the listener into the TUI. On other builds the subcommands exist in
// the CLI surface but report unavailability at runtime — matching how `watch`
// advertises itself when compiled out.

/// Bind the server socket for `serve`. The path is derived from the repo root
/// so a `push`/`decision` in the same repo finds it without an explicit flag.
/// `ServerArg` is a type alias for `ServerListener` under the `serve` feature,
/// so we return the listener directly.
#[cfg(all(feature = "serve", unix))]
fn spawn_serve_listener(repo: &std::path::Path) -> Result<next_hunk::tui::ServerArg> {
    let socket = next_hunk::cli_parse::runtime_socket_path(repo);
    next_hunk::tui::server::ServerListener::spawn(socket)
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_serve(
    _staged: bool,
    _watch: bool,
    _no_highlight: bool,
    _include_untracked: bool,
    _focus: Option<String>,
    _note: Vec<String>,
    _extra: Vec<String>,
) -> Result<()> {
    bail!("`serve` requires the `serve` feature on a Unix OS (rebuild with --features serve)");
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_push(_focus: Option<String>, _note: Vec<String>) -> Result<()> {
    bail!("`push` requires the `serve` feature on a Unix OS (rebuild with --features serve)");
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_decision() -> Result<()> {
    bail!("`decision` requires the `serve` feature on a Unix OS (rebuild with --features serve)")
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_list() -> Result<()> {
    bail!("`list` requires the `serve` feature on a Unix OS (rebuild with --features serve)")
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_get(_hash: Option<String>) -> Result<()> {
    bail!("`get` requires the `serve` feature on a Unix OS (rebuild with --features serve)")
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_review(_hash: Option<String>) -> Result<()> {
    bail!("`review` requires the `serve` feature on a Unix OS (rebuild with --features serve)")
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_navigate(_target: String, _hash: Option<String>) -> Result<()> {
    bail!("`navigate` requires the `serve` feature on a Unix OS (rebuild with --features serve)")
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_comment(_action: CommentAction) -> Result<()> {
    bail!("`comment` requires the `serve` feature on a Unix OS (rebuild with --features serve)")
}

#[cfg(not(all(feature = "serve", unix)))]
fn run_reload(_hash: Option<String>) -> Result<()> {
    bail!("`reload` requires the `serve` feature on a Unix OS (rebuild with --features serve)")
}

#[allow(clippy::too_many_arguments)]
fn open_review_from_text(
    text: &str,
    reloader: Option<next_hunk::tui::Reloader>,
    highlight_on: bool,
    line_numbers_on: bool,
    wrap_on: bool,
    theme: Option<String>,
    layout: next_hunk::config::LayoutMode,
    workdir: Option<PathBuf>,
    options: ReviewOptions,
    server: Option<next_hunk::tui::ServerArg>,
) -> Result<()> {
    if text.trim().is_empty() {
        eprintln!("(empty diff)");
        return Ok(());
    }
    let review = parse_review(text)?;
    let select_mode = options.select_mode;
    // Interactive TUI (Phase 2). If stdout is not a terminal (piped, e.g. when
    // used as git's pager in a pipeline or scripted in CI), or if opening the
    // TUI fails for any other reason, fall back to a short inspect summary so
    // the CLI path stays usable.
    //
    // We check `is_terminal()` explicitly rather than relying on crossterm to
    // error out, because on Windows crossterm's console calls can *block* on a
    // non-console (pipe) stdout instead of returning promptly - which would
    // hang the process (observed: `pager`/`patch -` deadlocked indefinitely in
    // piped integration tests). The upfront check is portable and avoids that.
    if !std::io::stdout().is_terminal() {
        print_inspect(&review);
        return Ok(());
    }
    match run_review_tui(
        review.clone(),
        reloader,
        highlight_on,
        line_numbers_on,
        wrap_on,
        theme,
        layout,
        workdir,
        options,
        server,
    ) {
        Ok(selections) => {
            // In --select mode the human's per-hunk decisions go to stdout as
            // JSON for the agent to parse. Outside --select, silently drop the
            // (empty) selections.
            if select_mode {
                println!("{}", serde_json::to_string(&selections)?);
            }
            Ok(())
        }
        Err(err) => {
            eprintln!("note: {err}");
            print_inspect(&review);
            Ok(())
        }
    }
}

fn parse_review(text: &str) -> Result<Review> {
    parse_unified_diff(text).context("failed to parse unified diff")
}

fn print_inspect(review: &Review) {
    println!(
        "files={} stream_rows={} arena_bytes={}",
        review.file_count(),
        review.stream_len,
        review.text_arena.len()
    );
    for (i, file) in review.files.iter().enumerate() {
        println!(
            "  [{i}] {}  hunks={} stream=[{}+{})",
            file.display_path,
            file.hunks.len(),
            file.stream_start,
            file.stream_len
        );
    }
}

fn read_patch_input(path: &std::path::Path) -> Result<String> {
    if path.as_os_str() == "-" {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .context("read patch from stdin")?;
        return Ok(buf);
    }
    if !path.exists() {
        bail!("patch file not found: {}", path.display());
    }
    std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}
