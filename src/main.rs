//! next-hunk CLI entry.

use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
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
        extra: Vec::new(),
    }) {
        Commands::Diff { staged, extra } => {
            let repo = find_repo(&std::env::current_dir()?)?;
            let text = git_diff(&repo, staged, &extra)?;
            open_review_from_text(&text)
        }
        Commands::Show { rev } => {
            let repo = find_repo(&std::env::current_dir()?)?;
            let text = git_show(&repo, &rev)?;
            open_review_from_text(&text)
        }
        Commands::Patch { path } => {
            let text = read_patch_input(&path)?;
            open_review_from_text(&text)
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
    }
}

fn open_review_from_text(text: &str) -> Result<()> {
    if text.trim().is_empty() {
        eprintln!("(empty diff)");
        return Ok(());
    }
    let review = parse_review(text)?;
    // Interactive TUI (Phase 2). If it fails (e.g. stdout is not a tty),
    // fall back to a short inspect summary so the CLI path stays usable.
    match run_review_tui(review.clone()) {
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
