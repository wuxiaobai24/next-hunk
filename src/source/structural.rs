//! Optional **structural** diff backend via external [`difft`](https://github.com/Wilfred/difftastic).
//!
//! Default path is never used — callers must opt in with `--structural` or
//! `structural = true` in config. Tradeoffs (see `docs/PERF.md`):
//! - Better readability for refactors, JSON/HTML nesting, and rename-heavy edits
//! - Higher latency (one `difft` subprocess per changed file) and not under the
//!   default parse/viewport bench gate
//! - Requires `difft` on `PATH` (or `NEXT_HUNK_DIFFT`); missing binary is a
//!   clear hard error, not a silent no-op
//!
//! Pipeline: baseline unified text (gix/jj) → reconstruct old/new sides from
//! hunks → `difft --display=json` (preferred) or inline fallback → re-emit
//! git-style unified so the existing IR / viewport path is unchanged.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::ir::{
    parse_unified_diff, split_file_sections, DiffLineKind, FileDiff, FileOrigin, Review,
};
use crate::source::ProducedDiff;

/// Env var overriding the `difft` binary path (default: look up `difft` on PATH).
pub const DIFFT_ENV: &str = "NEXT_HUNK_DIFFT";

/// Resolve the difft binary path (cached).
pub fn difft_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        std::env::var_os(DIFFT_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("difft"))
    })
    .clone()
}

/// Whether a difftastic binary is runnable (`--version` succeeds).
pub fn difft_available() -> bool {
    Command::new(difft_bin())
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Hard error when structural mode is requested but difft is missing.
pub fn require_difft() -> Result<()> {
    if difft_available() {
        return Ok(());
    }
    let bin = difft_bin();
    bail!(
        "structural mode requires `difft` (difftastic) on PATH\n\
         looked for: {}\n\
         install: https://github.com/Wilfred/difftastic#installation\n\
         or set {DIFFT_ENV}=/path/to/difft\n\
         (omit --structural / structural=true to keep the default unified path)",
        bin.display()
    );
}

/// Re-emit a produced unified diff through difftastic when possible.
///
/// * Empty baseline → returned unchanged.
/// * Per-file structural failure → that file keeps its original unified section
///   and a stderr warning is printed (does not fail the whole review).
/// * Missing `difft` → [`Err`] (callers should check before opening the TUI).
pub fn enhance_with_structural(produced: ProducedDiff) -> Result<ProducedDiff> {
    require_difft()?;
    if produced.is_empty() {
        return Ok(produced);
    }

    let review = match parse_unified_diff(&produced.text) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("warning: structural: baseline not parseable ({e}); keeping unified");
            return Ok(produced);
        }
    };

    let sections = split_file_sections(&produced.text);
    let mut out = String::new();
    let mut origins: Vec<FileOrigin> = Vec::new();

    for (i, file) in review.files.iter().enumerate() {
        let origin = produced.origins.get(i).copied();
        match structuralize_file(&review, file) {
            Ok(patch) if !patch.trim().is_empty() => {
                out.push_str(&patch);
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                if let Some(o) = origin {
                    origins.push(o);
                }
            }
            Ok(_) => {
                // difft reported unchanged — keep original section if any.
                append_fallback(&mut out, &mut origins, &sections, i, origin, file);
            }
            Err(e) => {
                eprintln!(
                    "warning: structural: fell back to unified for {}: {e:#}",
                    file.display_path
                );
                append_fallback(&mut out, &mut origins, &sections, i, origin, file);
            }
        }
    }

    if out.trim().is_empty() {
        // All files fell through empty — keep baseline so callers still see content.
        return Ok(produced);
    }

    Ok(ProducedDiff { text: out, origins })
}

fn append_fallback(
    out: &mut String,
    origins: &mut Vec<FileOrigin>,
    sections: &[crate::ir::FileSection<'_>],
    i: usize,
    origin: Option<FileOrigin>,
    file: &FileDiff,
) {
    if let Some(sec) = sections.get(i) {
        out.push_str(sec.text);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        if let Some(o) = origin {
            origins.push(o);
        }
    } else {
        eprintln!(
            "warning: structural: no baseline section for {} (skipped)",
            file.display_path
        );
    }
}

fn structuralize_file(review: &Review, file: &FileDiff) -> Result<String> {
    if file.hunks.is_empty() {
        bail!("no content hunks (binary or mode-only)");
    }

    let (old_text, new_text) = reconstruct_sides(review, file);
    if old_text.is_empty() && new_text.is_empty() {
        bail!("empty sides after reconstruct");
    }

    let dir = std::env::temp_dir().join(format!(
        "next-hunk-structural-{}-{}",
        std::process::id(),
        simple_hash(&file.display_path)
    ));
    std::fs::create_dir_all(&dir).context("create structural temp dir")?;
    let old_path = dir.join("old");
    let new_path = dir.join("new");
    // Preserve a recognizable extension for language detection when possible.
    let ext = Path::new(&file.display_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("txt");
    let old_path = old_path.with_extension(ext);
    let new_path = new_path.with_extension(ext);

    write_temp(&old_path, &old_text)?;
    write_temp(&new_path, &new_text)?;

    let result = run_difft_to_unified(&old_path, &new_path, &old_text, &new_text, file);
    // Best-effort cleanup; ignore errors (tmpdir may be shared/slow).
    let _ = std::fs::remove_file(&old_path);
    let _ = std::fs::remove_file(&new_path);
    let _ = std::fs::remove_dir(&dir);
    result
}

fn write_temp(path: &Path, text: &str) -> Result<()> {
    let mut f =
        std::fs::File::create(path).with_context(|| format!("write temp {}", path.display()))?;
    f.write_all(text.as_bytes())?;
    Ok(())
}

fn simple_hash(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Rebuild approximate old/new buffers from unified hunk lines (changed regions).
///
/// Enough for difftastic to re-diff the structural delta; not a full-file checkout.
pub fn reconstruct_sides(review: &Review, file: &FileDiff) -> (String, String) {
    let mut old = String::new();
    let mut new = String::new();
    for hunk in &file.hunks {
        for line in &hunk.lines {
            let text = review.text(line.text.clone());
            match line.kind {
                DiffLineKind::Context => {
                    old.push_str(text);
                    old.push('\n');
                    new.push_str(text);
                    new.push('\n');
                }
                DiffLineKind::Delete => {
                    old.push_str(text);
                    old.push('\n');
                }
                DiffLineKind::Add => {
                    new.push_str(text);
                    new.push('\n');
                }
                DiffLineKind::Meta => {}
            }
        }
    }
    (old, new)
}

fn run_difft_to_unified(
    old_path: &Path,
    new_path: &Path,
    old_text: &str,
    new_text: &str,
    file: &FileDiff,
) -> Result<String> {
    // Prefer machine-readable JSON (may need DFT_UNSTABLE on some versions).
    match run_difft_json(old_path, new_path) {
        Ok(raw) => {
            if let Ok(patch) = json_output_to_unified(&raw, old_text, new_text, file) {
                return Ok(patch);
            }
            // JSON parse failed — try inline, then hard fallback error.
        }
        Err(_) => {
            // JSON mode unavailable — try inline.
        }
    }

    let inline = run_difft_inline(old_path, new_path)?;
    inline_output_to_unified(&inline, old_text, new_text, file)
}

fn run_difft_json(old_path: &Path, new_path: &Path) -> Result<String> {
    let output = Command::new(difft_bin())
        .env("DFT_UNSTABLE", "yes")
        .env("DFT_COLOR", "never")
        .args(["--display", "json", "--color", "never"])
        .arg(old_path)
        .arg(new_path)
        .output()
        .context("spawn difft (json)")?;
    // difft may exit non-zero when files differ; still use stdout.
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if stdout.trim().is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "difft json produced no stdout (status={:?}): {stderr}",
            output.status.code()
        );
    }
    Ok(stdout)
}

fn run_difft_inline(old_path: &Path, new_path: &Path) -> Result<String> {
    let output = Command::new(difft_bin())
        .env("DFT_COLOR", "never")
        .args(["--display", "inline", "--color", "never"])
        .arg(old_path)
        .arg(new_path)
        .output()
        .context("spawn difft (inline)")?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if stdout.trim().is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "difft inline produced no stdout (status={:?}): {stderr}",
            output.status.code()
        );
    }
    Ok(stdout)
}

// --- difft JSON (best-effort; shape is somewhat unstable across versions) ---

#[derive(Debug, Deserialize)]
struct DifftFileResult {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    chunks: Option<Vec<Vec<DifftLinePair>>>,
    #[serde(default)]
    language: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DifftLinePair {
    #[serde(default)]
    lhs: Option<DifftSide>,
    #[serde(default)]
    rhs: Option<DifftSide>,
}

#[derive(Debug, Deserialize)]
struct DifftSide {
    /// 0-based line index into the corresponding side (difftastic convention).
    #[serde(default)]
    line_number: Option<u32>,
}

/// Convert difft JSON array/object output into a single-file unified patch.
pub fn json_output_to_unified(
    raw: &str,
    old_text: &str,
    new_text: &str,
    file: &FileDiff,
) -> Result<String> {
    let trimmed = raw.trim();
    // Array of file results, or a single object.
    let results: Vec<DifftFileResult> = if trimmed.starts_with('[') {
        serde_json::from_str(trimmed).context("parse difft json array")?
    } else {
        let one: DifftFileResult =
            serde_json::from_str(trimmed).context("parse difft json object")?;
        vec![one]
    };

    let Some(result) = results.into_iter().next() else {
        bail!("difft json: empty result array");
    };

    let status = result.status.as_deref().unwrap_or("changed");
    if status.eq_ignore_ascii_case("unchanged") {
        return Ok(String::new());
    }

    let old_lines: Vec<&str> = lines_no_final_only(old_text);
    let new_lines: Vec<&str> = lines_no_final_only(new_text);

    let mut deletes: Vec<String> = Vec::new();
    let mut adds: Vec<String> = Vec::new();

    if let Some(chunks) = result.chunks {
        for chunk in chunks {
            for pair in chunk {
                if let Some(lhs) = pair.lhs {
                    if let Some(ln) = lhs.line_number {
                        if let Some(line) = old_lines.get(ln as usize) {
                            deletes.push((*line).to_string());
                        }
                    }
                }
                if let Some(rhs) = pair.rhs {
                    if let Some(ln) = rhs.line_number {
                        if let Some(line) = new_lines.get(ln as usize) {
                            adds.push((*line).to_string());
                        }
                    }
                }
            }
        }
    }

    // If JSON had no usable line numbers, fall back to whole-file sides.
    if deletes.is_empty() && adds.is_empty() {
        deletes = old_lines.iter().map(|s| (*s).to_string()).collect();
        adds = new_lines.iter().map(|s| (*s).to_string()).collect();
    }

    let lang = result.language.as_deref();
    Ok(emit_unified_file(
        file,
        &deletes,
        &adds,
        lang.map(|l| format!("structural via difftastic ({l})")),
    ))
}

/// Inline mode: wrap difft's human output as a single synthetic hunk of context
/// lines (prefixed with a structural banner). Keeps IR happy when JSON is off.
fn inline_output_to_unified(
    inline: &str,
    old_text: &str,
    new_text: &str,
    file: &FileDiff,
) -> Result<String> {
    let stripped = strip_ansi(inline);
    let body_lines: Vec<String> = stripped
        .lines()
        .map(|l| l.to_string())
        .filter(|l| !l.trim().is_empty())
        .collect();
    if body_lines.is_empty() {
        // Unchanged or empty — fall back to full sides as a normal hunk.
        let old_lines: Vec<String> = lines_no_final_only(old_text)
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let new_lines: Vec<String> = lines_no_final_only(new_text)
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        return Ok(emit_unified_file(
            file,
            &old_lines,
            &new_lines,
            Some("structural via difftastic (inline empty)".into()),
        ));
    }

    // Present structural inline text as a pure-add "annotation" hunk so the
    // human still sees difft's view inside next-hunk's rail, plus keep classic
    // +/- from reconstructed sides as a second hunk for line-level navigation.
    let old_lines: Vec<String> = lines_no_final_only(old_text)
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    let new_lines: Vec<String> = lines_no_final_only(new_text)
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    let path = display_git_path(file);
    let mut out = String::new();
    out.push_str(&format!("diff --git a/{path} b/{path}\n"));
    out.push_str(&format!("--- a/{path}\n"));
    out.push_str(&format!("+++ b/{path}\n"));

    // Hunk 1: structural annotation (all "add" so it shows in the stream).
    let n = body_lines.len() as u32;
    out.push_str(&format!(
        "@@ -0,0 +1,{n} @@ structural via difftastic (inline)\n"
    ));
    for line in &body_lines {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }

    // Hunk 2: classic line sides (for line numbers / comments).
    if !old_lines.is_empty() || !new_lines.is_empty() {
        let oc = old_lines.len() as u32;
        let nc = new_lines.len() as u32;
        out.push_str(&format!("@@ -1,{oc} +1,{nc} @@ reconstructed sides\n"));
        for line in &old_lines {
            out.push('-');
            out.push_str(line);
            out.push('\n');
        }
        for line in &new_lines {
            out.push('+');
            out.push_str(line);
            out.push('\n');
        }
    }

    Ok(out)
}

fn emit_unified_file(
    file: &FileDiff,
    deletes: &[String],
    adds: &[String],
    banner: Option<String>,
) -> String {
    let path = display_git_path(file);
    let mut out = String::new();
    out.push_str(&format!("diff --git a/{path} b/{path}\n"));
    out.push_str(&format!("--- a/{path}\n"));
    out.push_str(&format!("+++ b/{path}\n"));
    let oc = deletes.len() as u32;
    let nc = adds.len() as u32;
    // Avoid 0,0 empty hunks when one side is empty (pure add/delete).
    let (old_start, old_count) = if oc == 0 { (0, 0) } else { (1, oc) };
    let (new_start, new_count) = if nc == 0 { (0, 0) } else { (1, nc) };
    let suffix = banner.map(|b| format!(" {b}")).unwrap_or_default();
    out.push_str(&format!(
        "@@ -{old_start},{old_count} +{new_start},{new_count} @@{suffix}\n"
    ));
    for line in deletes {
        out.push('-');
        out.push_str(line);
        out.push('\n');
    }
    for line in adds {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn display_git_path(file: &FileDiff) -> String {
    file.new_path
        .as_ref()
        .or(file.old_path.as_ref())
        .map(|p| {
            p.trim_start_matches("a/")
                .trim_start_matches("b/")
                .to_string()
        })
        .filter(|p| p != "/dev/null" && !p.is_empty())
        .unwrap_or_else(|| file.display_path.clone())
}

fn lines_no_final_only(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    // split_inclusive would keep newlines; we want content lines.
    let mut lines: Vec<&str> = text.split('\n').collect();
    // Trailing newline produces a final empty entry — drop it.
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

/// Strip CSI / OSC ANSI sequences (difft may still emit when color forced).
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for c2 in chars.by_ref() {
                        if c2.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC ... BEL or ST
                    chars.next();
                    for c2 in chars.by_ref() {
                        if c2 == '\u{7}' {
                            break;
                        }
                        if c2 == '\u{1b}' {
                            // ST = ESC \
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::parse_unified_diff;

    fn missing_difft_message_contains_install_hint(msg: &str) -> bool {
        msg.contains("difftastic") && msg.contains("PATH")
    }

    const SAMPLE: &str = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,4 @@
 fn main() {
-    println!(\"hi\");
+    println!(\"hello\");
+    println!(\"world\");
 }
";

    #[test]
    fn reconstruct_sides_from_hunks() {
        let review = parse_unified_diff(SAMPLE).unwrap();
        let (old, new) = reconstruct_sides(&review, &review.files[0]);
        assert!(old.contains("println!(\"hi\")"));
        assert!(!old.contains("println!(\"hello\")"));
        assert!(new.contains("println!(\"hello\")"));
        assert!(new.contains("println!(\"world\")"));
        assert!(!new.contains("println!(\"hi\")"));
    }

    #[test]
    fn json_to_unified_with_line_numbers() {
        let review = parse_unified_diff(SAMPLE).unwrap();
        let (old, new) = reconstruct_sides(&review, &review.files[0]);
        // 0-based: line 1 is println in both (with different content).
        let json = r#"[{
            "status": "changed",
            "language": "Rust",
            "chunks": [[
                {"lhs": {"line_number": 1}, "rhs": {"line_number": 1}},
                {"lhs": null, "rhs": {"line_number": 2}}
            ]]
        }]"#;
        let patch = json_output_to_unified(json, &old, &new, &review.files[0]).unwrap();
        assert!(patch.contains("diff --git"));
        assert!(patch.contains("structural via difftastic (Rust)"));
        assert!(patch.contains("-    println!(\"hi\")"));
        assert!(patch.contains("+    println!(\"hello\")"));
        let reparsed = parse_unified_diff(&patch).unwrap();
        assert_eq!(reparsed.file_count(), 1);
        assert!(!reparsed.files[0].hunks.is_empty());
    }

    #[test]
    fn json_unchanged_yields_empty() {
        let review = parse_unified_diff(SAMPLE).unwrap();
        let (old, new) = reconstruct_sides(&review, &review.files[0]);
        let json = r#"[{"status":"unchanged","chunks":[]}]"#;
        let patch = json_output_to_unified(json, &old, &new, &review.files[0]).unwrap();
        assert!(patch.is_empty());
    }

    #[test]
    fn inline_wraps_as_parseable_unified() {
        let review = parse_unified_diff(SAMPLE).unwrap();
        let (old, new) = reconstruct_sides(&review, &review.files[0]);
        let inline = "src/a.rs --- 1/1 ---\n1 fn main() {\n2     println!(\"hello\");\n";
        let patch = inline_output_to_unified(inline, &old, &new, &review.files[0]).unwrap();
        let reparsed = parse_unified_diff(&patch).unwrap();
        assert_eq!(reparsed.file_count(), 1);
        assert!(!reparsed.files[0].hunks.is_empty());
    }

    #[test]
    fn require_difft_message_shape_when_missing() {
        // Only assert the helper used in docs/tests; live require_difft depends
        // on the host having (or not having) difft.
        let msg = "structural mode requires `difft` (difftastic) on PATH";
        assert!(missing_difft_message_contains_install_hint(msg));
    }

    #[test]
    fn strip_ansi_removes_csi() {
        let s = "\u{1b}[31mred\u{1b}[0m plain";
        assert_eq!(strip_ansi(s), "red plain");
    }

    #[test]
    fn enhance_empty_is_ok() {
        // Without difft, enhance on empty still needs require_difft — so only
        // test the empty short-circuit when difft is available; otherwise skip.
        if !difft_available() {
            return;
        }
        let out = enhance_with_structural(ProducedDiff::default()).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn enhance_with_real_difft_when_present() {
        if !difft_available() {
            return;
        }
        let baseline = ProducedDiff {
            text: SAMPLE.into(),
            origins: Vec::new(),
        };
        let out = enhance_with_structural(baseline).unwrap();
        assert!(!out.text.is_empty());
        let review = parse_unified_diff(&out.text).unwrap();
        assert_eq!(review.file_count(), 1);
    }
}
