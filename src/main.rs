//! `next-hunk` binary entry: a thin shim over [`next_hunk::cli`].

fn main() -> std::process::ExitCode {
    next_hunk::cli::main()
}
