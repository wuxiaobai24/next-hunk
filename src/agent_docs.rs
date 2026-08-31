//! The agent-facing surface of the CLI: the embedded skill document and the
//! commands that expose it (`skill path`, `--agent-context`), plus the
//! `update --check` release probe.
//!
//! The skill document ships inside the binary (`include_str!`), so agents
//! never depend on a source checkout. `skill path` materializes it under
//! `$XDG_DATA_HOME/next-hunk/skill/SKILL.md` (first use) and prints the
//! path — the same contract as hunk's `hunk skill path`.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// The bundled agent skill (frontmatter + workflow), as shipped in
/// `skill/next-hunk/SKILL.md`.
pub const SKILL_DOC: &str = include_str!("../skill/next-hunk/SKILL.md");

/// Where `skill path` materializes the embedded document. Prefers
/// `$XDG_DATA_HOME/next-hunk`, falls back to `$HOME/.local/share/next-hunk`.
pub fn skill_path() -> Result<PathBuf> {
    let base = match std::env::var("XDG_DATA_HOME") {
        Ok(x) if !x.is_empty() => PathBuf::from(x),
        _ => {
            let home = std::env::var("HOME").context(
                "neither $XDG_DATA_HOME nor $HOME is set; nowhere to materialize the skill",
            )?;
            PathBuf::from(home).join(".local/share")
        }
    }
    .join("next-hunk")
    .join("skill");
    std::fs::create_dir_all(&base).context("create skill dir")?;
    let path = base.join("SKILL.md");
    // Refresh when the embedded copy is newer (binary upgraded). A write
    // failure is non-fatal: print the path we would have used.
    let stale = match std::fs::read_to_string(&path) {
        Ok(existing) => existing != SKILL_DOC,
        Err(_) => true,
    };
    if stale {
        if let Err(e) = std::fs::write(&path, SKILL_DOC) {
            anyhow::bail!("write {}: {e}", path.display());
        }
    }
    Ok(path)
}

/// Print the agent workflow document to stdout (`--agent-context`).
pub fn print_agent_context() {
    print!("{SKILL_DOC}");
}

// ─── update ───────────────────────────────────────────────────────────────────

/// GitHub repo that hosts releases (also the `cargo install --git` source).
pub const RELEASES_API: &str = "https://api.github.com/repos/wuxiaobai24/next-hunk/releases/latest";
pub const INSTALL_HINT: &str =
    "curl -fsSL https://github.com/wuxiaobai24/next-hunk/raw/main/scripts/install.sh | bash";

/// Compare a release tag (`v0.5.0`) against the running version
/// (`env!("CARGO_PKG_VERSION")`). Returns `Some(latest)` when the tag is
/// strictly newer.
pub fn newer_release_tag(tag: &str, current: &str) -> Option<String> {
    fn strip(v: &str) -> &str {
        v.trim().trim_start_matches('v')
    }
    fn parse(v: &str) -> Vec<u64> {
        v.split('.')
            .map(|p| {
                p.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    }
    let (a, b) = (parse(strip(tag)), parse(strip(current)));
    if a > b {
        Some(strip(tag).to_string())
    } else {
        None
    }
}

/// `nh update [--check]`: probe the latest GitHub release.
/// `--check` only reports; without it we also print the install route(s)
/// (we never overwrite our own binary — the installer owns that).
pub fn update(check_only: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let tag = fetch_latest_tag().context(
        "could not reach GitHub to check for updates (offline? rate-limited?)",
    )?;
    match newer_release_tag(&tag, current) {
        Some(newer) => {
            println!("update available: v{newer} (installed: v{current})");
            if !check_only {
                println!();
                println!("one-line install (Linux x86_64):");
                println!("  {INSTALL_HINT}");
                println!("or from source:");
                println!("  cargo install --git https://github.com/wuxiaobai24/next-hunk");
            }
            Ok(())
        }
        None => {
            println!("up to date (v{current}, latest release: v{})", tag.trim_start_matches('v'));
            Ok(())
        }
    }
}

/// Fetch the latest release tag name from the GitHub API. Requires a TLS
/// client at minimum; we shell out to `curl` to avoid a heavy dependency —
/// every machine that installed via install.sh has it.
fn fetch_latest_tag() -> Result<String> {
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "10", RELEASES_API])
        .output()
        .context("spawn curl (is it installed?)")?;
    if !out.status.success() {
        anyhow::bail!("GitHub API request failed (exit {})", out.status);
    }
    let body = String::from_utf8_lossy(&out.stdout);
    // Minimal JSON scrape: "tag_name": "v0.5.0" — avoids a serde_json
    // dependency for one field.
    let key = "\"tag_name\"";
    let idx = body
        .find(key)
        .with_context(|| format!("no tag_name in release payload: {}", &body[..body.len().min(120)]))?;
    let rest = &body[idx + key.len()..];
    let start = rest.find('"').with_context(|| "malformed tag_name")? + 1;
    let end = rest[start..].find('"').with_context(|| "malformed tag_name")? + start;
    Ok(rest[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_release_comparison() {
        assert_eq!(newer_release_tag("v0.5.0", "0.4.0"), Some("0.5.0".into()));
        assert_eq!(newer_release_tag("0.4.1", "0.4.0"), Some("0.4.1".into()));
        assert_eq!(newer_release_tag("v1.0.0", "0.44.0"), Some("1.0.0".into()));
        // equal / older → None
        assert_eq!(newer_release_tag("v0.4.0", "0.4.0"), None);
        assert_eq!(newer_release_tag("v0.3.9", "0.4.0"), None);
        // pre-release-ish tags compare on the numeric prefix
        assert_eq!(newer_release_tag("v0.5.0-rc.1", "0.4.0"), Some("0.5.0-rc.1".into()));
    }

    #[test]
    fn skill_doc_embedded_nonempty() {
        assert!(SKILL_DOC.contains("next-hunk"));
        assert!(SKILL_DOC.starts_with("---"), "frontmatter preserved");
    }
}
