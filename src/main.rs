//! next-hunk CLI entry.

use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use next_hunk::config::{CliFlags, Config, ResolvedConfig};
use next_hunk::ir::{parse_unified_diff, Review};
use next_hunk::source::{find_repo, git_diff, git_show};
use next_hunk::tui::run_review_tui;

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
        /// Optional pathspecs to limit the review.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        extra: Vec<String>,
    },
    /// Review a commit or range (`git show` / `git diff A..B`).
    Show {
        /// Revision or range (e.g. HEAD, main..HEAD).
        rev: String,
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
        extra: Vec::new(),
    }) {
        Commands::Diff {
            staged,
            watch,
            no_highlight,
            extra,
        } => {
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
                },
            );

            if resolved.watch && !next_hunk::tui::watch::Watcher::is_enabled() {
                eprintln!(
                    "note: `--watch` requires the `watch` feature (rebuild with --features watch)"
                );
            }

            let text = git_diff(&repo, resolved.staged, &extra)?;
            let reloader = if resolved.watch {
                Some(make_diff_reloader(repo.clone(), resolved.staged, extra))
            } else {
                None
            };
            open_review_from_text(
                &text,
                reloader,
                resolved.highlight,
                resolved.theme,
                Some(repo),
            )
        }
        Commands::Show { rev } => {
            let cwd = std::env::current_dir()?;
            let repo = find_repo(&cwd)?;
            let text = git_show(&repo, &rev)?;
            // `show` is a one-shot snapshot: no watch, highlight default on.
            // Honor the user/project theme config even for `show`.
            let cfg = Config::load(&cwd);
            open_review_from_text(&text, None, true, cfg.theme, Some(repo))
        }
        Commands::Patch { path } => {
            let text = read_patch_input(&path)?;
            open_review_from_text(&text, None, true, None, None)
        }
        Commands::Inspect { path, staged } => {
            let text = if let Some(path) = path {
                read_patch_input(&path)?
            } else {
                let repo = find_repo(&std::env::current_dir()?)?;
                git_diff(&repo, staged, &[])?
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
            open_review_from_text(&buf, None, true, cfg.theme, workdir)
        }
    }
}

/// Build the live-reload closure for `--watch`: re-runs the same git diff.
/// Captures the repo path, staged flag, and pathspecs by value.
fn make_diff_reloader(repo: PathBuf, staged: bool, extra: Vec<String>) -> next_hunk::tui::Reloader {
    Box::new(move || git_diff(&repo, staged, &extra).context("re-run git diff for --watch"))
}

fn open_review_from_text(
    text: &str,
    reloader: Option<next_hunk::tui::Reloader>,
    highlight_on: bool,
    theme: Option<String>,
    workdir: Option<PathBuf>,
) -> Result<()> {
    if text.trim().is_empty() {
        eprintln!("(empty diff)");
        return Ok(());
    }
    let review = parse_review(text)?;
    // Interactive TUI (Phase 2). If it fails (e.g. stdout is not a tty),
    // fall back to a short inspect summary so the CLI path stays usable.
    match run_review_tui(review.clone(), reloader, highlight_on, theme, workdir) {
        Ok(()) => Ok(()),
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
