//! `nh` — short-alias binary for next-hunk (same program, shorter name).
//!
//! The CLI names itself after argv[0], so usage and error output show `nh`.

fn main() -> std::process::ExitCode {
    next_hunk::cli::main()
}
