//! Layered configuration: user + project `config.toml`, merged with CLI flags.
//!
//! Precedence (highest wins):
//! ```text
//! CLI flag  >  .next-hunk/config.toml (project)  >  ~/.config/next-hunk/config.toml (user)  >  defaults
//! ```
//!
//! All config fields are `Option<T>` so an absent key is distinct from an
//! explicit `false` — that is what lets "lower layer sets it, upper layer
//! leaves it" compose correctly.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Which VCS backend to use for repository-backed commands.
///
/// Default [`VcsPreference::Auto`] prefers Jujutsu when a `.jj` workspace is
/// present (including colocated git+jj trees); otherwise git via gix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VcsPreference {
    /// Prefer jj when `.jj` is present; otherwise git.
    #[default]
    Auto,
    /// Force the gix (gitoxide) adapter.
    Git,
    /// Force the `jj` CLI adapter (no git compatibility layer required).
    Jj,
}

impl VcsPreference {
    pub fn as_str(self) -> &'static str {
        match self {
            VcsPreference::Auto => "auto",
            VcsPreference::Git => "git",
            VcsPreference::Jj => "jj",
        }
    }

    /// Parse config/CLI values. Unknown → `Auto`.
    pub fn parse_str(s: &str) -> Self {
        Self::try_parse(s).unwrap_or(VcsPreference::Auto)
    }

    /// Parse config values. Unknown → error with the allowed set.
    pub fn try_parse(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "git" => Ok(VcsPreference::Git),
            "jj" | "jujutsu" => Ok(VcsPreference::Jj),
            "auto" | "" => Ok(VcsPreference::Auto),
            other => Err(format!("unknown vcs '{other}' (expected auto, git, or jj)")),
        }
    }
}

/// Which local git buckets to include in a `diff` / `serve` / `inspect` review.
///
/// Default stays [`DiffScope::Worktree`] (unstaged only) so existing muscle
/// memory matches `git diff`. Use [`DiffScope::WorkingSet`] (`--all`) to see
/// staged + unstaged (+ optional untracked) in one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffScope {
    /// Index vs worktree (`git diff`). Optional untracked via `include_untracked`.
    #[default]
    Worktree,
    /// HEAD vs index (`git diff --cached` / `--staged`).
    Staged,
    /// Staged + unstaged (+ optional untracked). CLI: `--all`.
    WorkingSet,
}

impl DiffScope {
    pub fn as_str(self) -> &'static str {
        match self {
            DiffScope::Worktree => "worktree",
            DiffScope::Staged => "staged",
            DiffScope::WorkingSet => "working-set",
        }
    }

    /// Parse config values. Unknown → error with the allowed set.
    pub fn try_parse(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "staged" | "cached" | "index" => Ok(DiffScope::Staged),
            "working-set" | "working_set" | "all" | "ws" => Ok(DiffScope::WorkingSet),
            "worktree" | "unstaged" | "wt" => Ok(DiffScope::Worktree),
            other => Err(format!(
                "unknown scope '{other}' (expected worktree, staged, or working-set)"
            )),
        }
    }

    /// Parse config values. Unknown → `Worktree` (lenient default for tests).
    pub fn parse_str(s: &str) -> Self {
        Self::try_parse(s).unwrap_or(DiffScope::Worktree)
    }
}

/// Layout mode for the diff stream pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutMode {
    /// Unified diff: interleaved context/add/delete per hunk (traditional).
    #[default]
    Unified,
    /// Stacked diff: old content (context + deletes) then new content
    /// (context + adds) per file, vertically.
    Stack,
    /// Side-by-side split: old (left) and new (right) panes for the same file.
    /// Narrow terminals fall back to stack, then unified (see view layer).
    Split,
}

/// What to print on TUI quit for agents (and humans pasting into chat).
///
/// Default is [`ExportOnQuit::None`] so everyday pager/`git diff` use does not
/// pollute stdout. `--select` still emits decision JSON when export is `none`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportOnQuit {
    /// No export (except legacy `--select` decisions-only JSON).
    #[default]
    None,
    /// One JSON line: decisions + comments + notes (superset of `decision`).
    Json,
    /// Human/agent-readable Markdown report.
    Markdown,
    /// JSON line, then Markdown body.
    Both,
}

impl ExportOnQuit {
    pub fn as_str(self) -> &'static str {
        match self {
            ExportOnQuit::None => "none",
            ExportOnQuit::Json => "json",
            ExportOnQuit::Markdown => "markdown",
            ExportOnQuit::Both => "both",
        }
    }

    /// Parse config/CLI values. Unknown → error with the allowed set.
    pub fn try_parse(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "none" => Ok(ExportOnQuit::None),
            "json" => Ok(ExportOnQuit::Json),
            "markdown" | "md" => Ok(ExportOnQuit::Markdown),
            "both" => Ok(ExportOnQuit::Both),
            other => Err(format!(
                "unknown export_on_quit '{other}' (expected none, json, markdown, or both)"
            )),
        }
    }

    /// Parse config/CLI values. Unknown → `None` (lenient default for tests).
    pub fn parse_str(s: &str) -> Self {
        Self::try_parse(s).unwrap_or(ExportOnQuit::None)
    }
}

impl LayoutMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            LayoutMode::Unified => "unified",
            LayoutMode::Stack => "stack",
            LayoutMode::Split => "split",
        }
    }

    /// Parse layout mode. Unknown → error with the allowed set.
    pub fn try_parse(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "unified" => Ok(LayoutMode::Unified),
            "stack" => Ok(LayoutMode::Stack),
            "split" => Ok(LayoutMode::Split),
            other => Err(format!(
                "unknown layout '{other}' (expected unified, stack, or split)"
            )),
        }
    }

    /// Parse layout mode. Unknown → `Unified` (lenient default for tests).
    pub fn parse_str(s: &str) -> Self {
        Self::try_parse(s).unwrap_or(LayoutMode::Unified)
    }
}

/// Raw user-configurable options. Every field optional: `None` = "not set".
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    /// Legacy: `staged = true` maps to [`DiffScope::Staged`] when `scope` is unset.
    pub staged: Option<bool>,
    /// Diff bucket scope: `"worktree"` | `"staged"` | `"working-set"`.
    /// Preferred over `staged` when both are set.
    pub scope: Option<String>,
    pub highlight: Option<bool>,
    pub watch: Option<bool>,
    /// Show a line-number gutter column.
    pub line_numbers: Option<bool>,
    /// Include untracked files in worktree / working-set diff.
    pub include_untracked: Option<bool>,
    /// TUI theme name: "dark" / "light" / "auto" (auto = detect via $COLORFGBG).
    pub theme: Option<String>,
    /// Layout mode: "unified" (default), "stack", or "split".
    pub layout: Option<String>,
    /// Wrap long lines in the diff stream pane. `false` = truncate (default).
    pub wrap: Option<bool>,
    /// On quit, emit an agent-readable report: "none" | "json" | "markdown" | "both".
    pub export_on_quit: Option<String>,
    /// VCS backend: `"auto"` | `"git"` | `"jj"`. Default auto prefers jj when a
    /// `.jj` workspace is present (including colocated git+jj repos).
    pub vcs: Option<String>,
    /// Persist per-hunk accept/reject decisions across sessions (default true).
    /// Stored under `.git/next-hunk/decisions-<scope>.json`.
    pub persist_review: Option<bool>,
}

impl Config {
    /// Merge `other` into `self`, with `other` (the higher-precedence layer)
    /// winning wherever it is `Some`. Returns `self` for chaining.
    pub fn merge(mut self, other: Config) -> Config {
        if other.staged.is_some() {
            self.staged = other.staged;
        }
        if other.scope.is_some() {
            self.scope = other.scope;
        }
        if other.highlight.is_some() {
            self.highlight = other.highlight;
        }
        if other.watch.is_some() {
            self.watch = other.watch;
        }
        if other.line_numbers.is_some() {
            self.line_numbers = other.line_numbers;
        }
        if other.include_untracked.is_some() {
            self.include_untracked = other.include_untracked;
        }
        if other.theme.is_some() {
            self.theme = other.theme;
        }
        if other.layout.is_some() {
            self.layout = other.layout;
        }
        if other.wrap.is_some() {
            self.wrap = other.wrap;
        }
        if other.export_on_quit.is_some() {
            self.export_on_quit = other.export_on_quit;
        }
        if other.vcs.is_some() {
            self.vcs = other.vcs;
        }
        if other.persist_review.is_some() {
            self.persist_review = other.persist_review;
        }
        self
    }

    /// Load the user-level config from `~/.config/next-hunk/config.toml`
    /// (honoring `$XDG_CONFIG_HOME` and `$HOME`). Missing file = empty config.
    ///
    /// Returns `Err` when the file exists but is unreadable, not valid TOML, or
    /// contains an illegal enum value (so startup fails loudly instead of
    /// silently ignoring a typo).
    pub fn load_user() -> Result<Config, String> {
        match user_config_path() {
            Some(p) => Ok(load_file(&p)?.unwrap_or_default()),
            None => Ok(Config::default()),
        }
    }

    /// Load the project-level config by walking up from `start` looking for a
    /// `.next-hunk/config.toml`. Missing = empty config. See [`Self::load_user`]
    /// for error semantics on a present-but-invalid file.
    pub fn load_project(start: &Path) -> Result<Config, String> {
        match find_project_config(start) {
            Some(p) => Ok(load_file(&p)?.unwrap_or_default()),
            None => Ok(Config::default()),
        }
    }

    /// Load the full layered config: user merged with project (project wins).
    pub fn load(start: &Path) -> Result<Config, String> {
        Ok(Config::load_user()?.merge(Config::load_project(start)?))
    }
}

/// The effective values after merging config layers with CLI flags.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    /// Which local change buckets to review.
    pub scope: DiffScope,
    pub highlight: bool,
    pub watch: bool,
    /// Show a line-number gutter column. ON by default.
    pub line_numbers: bool,
    /// Include untracked files in worktree / working-set diff. OFF by default (safe).
    pub include_untracked: bool,
    /// TUI theme name ("dark" / "light" / "auto"). `None` = use the app default
    /// (dark). Config-only in this pass — no CLI flag yet.
    pub theme: Option<String>,
    /// Layout mode for the diff stream: "unified" (default), "stack", or "split".
    pub layout: LayoutMode,
    /// Wrap long lines in the diff stream pane. `false` = truncate (default).
    pub wrap: bool,
    /// Emit a structured review report when the TUI quits.
    pub export_on_quit: ExportOnQuit,
    /// Which VCS backend to use for `diff` / `show` / `serve` / `inspect`.
    pub vcs: VcsPreference,
    /// Persist per-hunk decisions across sessions. ON by default.
    pub persist_review: bool,
}

impl Default for ResolvedConfig {
    fn default() -> Self {
        // Defaults: highlight on (matches existing TUI behavior), worktree/watch off.
        Self {
            scope: DiffScope::Worktree,
            highlight: true,
            watch: false,
            line_numbers: true,
            include_untracked: false,
            theme: None,
            layout: LayoutMode::Unified,
            wrap: false,
            export_on_quit: ExportOnQuit::None,
            vcs: VcsPreference::Auto,
            persist_review: true,
        }
    }
}

/// Inputs from the CLI for a single toggleable option.
///
/// Most options are "default unless overridden", so a simple `Option<bool>`
/// (CLI sets `Some(false)` for `--no-flag`, `Some(true)` for `--flag`) composes
/// cleanly with the config layer.
#[derive(Debug, Clone, Default)]
pub struct CliFlags {
    /// `--staged` / no flag. Mutually exclusive with `--all` at the clap layer.
    pub staged: Option<bool>,
    /// `--all` → full working-set (staged + unstaged).
    pub all: Option<bool>,
    /// `--watch` / no flag.
    pub watch: Option<bool>,
    /// `--no-highlight` → `Some(false)`; absent → `None`.
    pub highlight: Option<bool>,
    /// `--include-untracked` → `Some(true)`; absent → `None`.
    pub include_untracked: Option<bool>,
    /// `--layout <mode>` → `Some(LayoutMode)`; absent → `None` (use config/default).
    pub layout: Option<LayoutMode>,
    /// `--export-on-quit <mode>` → `Some(ExportOnQuit)`; absent → `None`.
    pub export_on_quit: Option<ExportOnQuit>,
    /// `--vcs <auto|git|jj>` → `Some(VcsPreference)`; absent → `None`.
    pub vcs: Option<VcsPreference>,
    /// `--no-persist` → `Some(false)`; absent → `None` (use config/default).
    pub persist_review: Option<bool>,
}

impl ResolvedConfig {
    /// Resolve the final config.
    ///
    /// CLI `Some` wins; otherwise the merged config; otherwise defaults.
    /// Illegal enum strings in config (e.g. `layout = "sidebyside"`) return
    /// `Err` with the field name and allowed values — never silent fallback.
    pub fn resolve(cfg: &Config, cli: &CliFlags) -> Result<Self, String> {
        let d = Self::default();
        if let Some(ref theme) = cfg.theme {
            validate_theme(theme)?;
        }
        Ok(Self {
            scope: resolve_scope(cfg, cli)?,
            highlight: cli.highlight.or(cfg.highlight).unwrap_or(d.highlight),
            watch: cli.watch.or(cfg.watch).unwrap_or(d.watch),
            line_numbers: cfg.line_numbers.unwrap_or(d.line_numbers),
            include_untracked: cli
                .include_untracked
                .or(cfg.include_untracked)
                .unwrap_or(d.include_untracked),
            theme: cfg.theme.clone(),
            layout: match cli.layout {
                Some(l) => l,
                None => match cfg.layout.as_deref() {
                    Some(s) => LayoutMode::try_parse(s)?,
                    None => d.layout,
                },
            },
            wrap: cfg.wrap.unwrap_or(d.wrap),
            export_on_quit: match cli.export_on_quit {
                Some(e) => e,
                None => match cfg.export_on_quit.as_deref() {
                    Some(s) => ExportOnQuit::try_parse(s)?,
                    None => d.export_on_quit,
                },
            },
            vcs: match cli.vcs {
                Some(v) => v,
                None => match cfg.vcs.as_deref() {
                    Some(s) => VcsPreference::try_parse(s)?,
                    None => d.vcs,
                },
            },
            persist_review: cli
                .persist_review
                .or(cfg.persist_review)
                .unwrap_or(d.persist_review),
        })
    }
}

/// Resolve [`DiffScope`] from CLI + config.
///
/// Precedence:
/// 1. CLI `--all` → `WorkingSet`
/// 2. CLI `--staged` → `Staged`
/// 3. Config `scope = "..."`
/// 4. Config `staged = true` → `Staged` (legacy)
/// 5. Default `Worktree`
fn resolve_scope(cfg: &Config, cli: &CliFlags) -> Result<DiffScope, String> {
    if cli.all == Some(true) {
        return Ok(DiffScope::WorkingSet);
    }
    if cli.staged == Some(true) {
        return Ok(DiffScope::Staged);
    }
    // Explicit CLI staged=false (shouldn't happen with a pure flag) keeps config.
    if let Some(scope) = cfg.scope.as_deref() {
        return DiffScope::try_parse(scope);
    }
    if cfg.staged == Some(true) {
        return Ok(DiffScope::Staged);
    }
    Ok(DiffScope::Worktree)
}

fn validate_theme(theme: &str) -> Result<(), String> {
    match theme.trim().to_lowercase().as_str() {
        "dark" | "light" | "auto" => Ok(()),
        other => Err(format!(
            "unknown theme '{other}' (expected dark, light, or auto)"
        )),
    }
}

// ─── file discovery ───────────────────────────────────────────────────────────

/// Resolve the user config path: `$XDG_CONFIG_HOME/next-hunk/config.toml` or
/// `$HOME/.config/next-hunk/config.toml`. `None` if neither var is set.
fn user_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("next-hunk").join("config.toml"));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config/next-hunk/config.toml"))
}

/// Walk up from `start` to find the nearest `.next-hunk/config.toml`.
fn find_project_config(start: &Path) -> Option<PathBuf> {
    let mut dir = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    loop {
        let candidate = dir.join(".next-hunk/config.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Read + parse a config file.
///
/// * Missing file → `Ok(None)` (common case, silent).
/// * Present but unreadable / invalid TOML / illegal enum → `Err` with a clear
///   message so the process exits non-zero instead of silently ignoring typos
///   like `layout = "sidebyside"`.
fn load_file(path: &Path) -> Result<Option<Config>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(e) => {
            return Err(format!("cannot read config {}: {e}", path.display()));
        }
    };
    let cfg: Config =
        toml::from_str(&text).map_err(|e| format!("invalid config {}: {e}", path.display()))?;
    validate_config_enums(&cfg, path)?;
    Ok(Some(cfg))
}

/// Fail fast on illegal string-enum fields so a typo never becomes a silent
/// default (dogfood: `layout = "sidebyside"` used to be ignored with exit 0).
fn validate_config_enums(cfg: &Config, path: &Path) -> Result<(), String> {
    let loc = path.display();
    if let Some(ref s) = cfg.layout {
        LayoutMode::try_parse(s).map_err(|e| format!("{loc}: {e}"))?;
    }
    if let Some(ref s) = cfg.scope {
        DiffScope::try_parse(s).map_err(|e| format!("{loc}: {e}"))?;
    }
    if let Some(ref s) = cfg.export_on_quit {
        ExportOnQuit::try_parse(s).map_err(|e| format!("{loc}: {e}"))?;
    }
    if let Some(ref s) = cfg.vcs {
        VcsPreference::try_parse(s).map_err(|e| format!("{loc}: {e}"))?;
    }
    if let Some(ref s) = cfg.theme {
        validate_theme(s).map_err(|e| format!("{loc}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_tmp_config(contents: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new();
        let cfg_dir = dir.0.join(".next-hunk");
        fs::create_dir_all(&cfg_dir).unwrap();
        let path = cfg_dir.join("config.toml");
        fs::write(&path, contents).unwrap();
        (dir, path)
    }

    /// Minimal temp-dir helper (avoids pulling in the `tempfile` crate).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "next-hunk-cfg-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn merge_higher_layer_wins() {
        let lower = Config {
            staged: Some(false),
            highlight: Some(true),
            watch: Some(false),
            ..Default::default()
        };
        let upper = Config {
            staged: Some(true),
            ..Default::default()
        };
        let merged = lower.merge(upper);
        assert_eq!(merged.staged, Some(true)); // upper wins
        assert_eq!(merged.highlight, Some(true)); // lower kept (upper None)
        assert_eq!(merged.watch, Some(false)); // lower kept
    }

    #[test]
    fn merge_none_keeps_lower() {
        let lower = Config {
            theme: Some("dark".into()),
            ..Default::default()
        };
        let upper = Config::default(); // all None
        let merged = lower.merge(upper);
        assert_eq!(merged.theme.as_deref(), Some("dark"));
    }

    #[test]
    fn resolve_cli_overrides_config() {
        let cfg = Config {
            staged: Some(true),
            highlight: Some(false),
            watch: Some(true),
            ..Default::default()
        };
        // --all wins over config staged=true
        let cli = CliFlags {
            all: Some(true),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert_eq!(r.scope, DiffScope::WorkingSet);
        assert!(!r.highlight); // config wins (CLI None)
        assert!(r.watch); // config wins
    }

    #[test]
    fn resolve_cli_staged_overrides_config_scope() {
        let cfg = Config {
            scope: Some("worktree".into()),
            ..Default::default()
        };
        let cli = CliFlags {
            staged: Some(true),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert_eq!(r.scope, DiffScope::Staged);
    }

    #[test]
    fn resolve_config_scope_working_set() {
        let cfg = Config {
            scope: Some("working-set".into()),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &CliFlags::default()).unwrap();
        assert_eq!(r.scope, DiffScope::WorkingSet);
    }

    #[test]
    fn resolve_legacy_staged_config() {
        let cfg = Config {
            staged: Some(true),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &CliFlags::default()).unwrap();
        assert_eq!(r.scope, DiffScope::Staged);
    }

    #[test]
    fn resolve_defaults_when_nothing_set() {
        let cfg = Config::default();
        let r = ResolvedConfig::resolve(&cfg, &CliFlags::default()).unwrap();
        assert_eq!(r.scope, DiffScope::Worktree);
        assert!(r.highlight); // default on
        assert!(!r.watch);
        assert!(r.line_numbers); // default on
    }

    #[test]
    fn resolve_no_highlight_flag_disables() {
        let cfg = Config {
            highlight: Some(true),
            ..Default::default()
        };
        let cli = CliFlags {
            highlight: Some(false), // --no-highlight
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert!(!r.highlight);
    }

    #[test]
    fn resolve_line_numbers_from_config() {
        let cfg = Config {
            line_numbers: Some(false),
            ..Default::default()
        };
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert!(!r.line_numbers); // config false wins
    }

    #[test]
    fn resolve_line_numbers_defaults_to_true() {
        let cfg = Config::default(); // line_numbers = None
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert!(r.line_numbers); // default on
    }

    #[test]
    fn resolve_carries_theme_from_config() {
        let cfg = Config {
            theme: Some("light".into()),
            ..Default::default()
        };
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert_eq!(r.theme.as_deref(), Some("light"));
    }

    #[test]
    fn resolve_theme_none_when_unset() {
        let cfg = Config::default();
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert!(r.theme.is_none());
    }

    #[test]
    fn parse_full_config() {
        let (dir, _path) = write_tmp_config(
            "\
staged = true
highlight = false
watch = true
line_numbers = true
theme = \"dark\"
",
        );
        let cfg = Config::load_project(&dir.0).unwrap();
        assert_eq!(cfg.staged, Some(true));
        assert_eq!(cfg.highlight, Some(false));
        assert_eq!(cfg.watch, Some(true));
        assert_eq!(cfg.line_numbers, Some(true));
        assert_eq!(cfg.theme.as_deref(), Some("dark"));
    }

    #[test]
    fn parse_partial_config() {
        let (dir, _path) = write_tmp_config("highlight = false\n");
        let cfg = Config::load_project(&dir.0).unwrap();
        assert_eq!(cfg.highlight, Some(false));
        assert_eq!(cfg.staged, None); // unset
    }

    #[test]
    fn parse_unknown_field_is_ignored() {
        // Unknown keys shouldn't break parsing (forward-compat).
        let (dir, _path) = write_tmp_config("highlight = true\nfuture_field = 42\n");
        let cfg = Config::load_project(&dir.0).unwrap();
        assert_eq!(cfg.highlight, Some(true));
    }

    #[test]
    fn find_project_config_walks_up() {
        let (dir, _path) = write_tmp_config("highlight = false\n");
        // create a nested subdir; config is found by walking up
        let nested = dir.0.join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        let cfg = Config::load_project(&nested).unwrap();
        assert_eq!(cfg.highlight, Some(false));
    }

    #[test]
    fn missing_config_returns_empty() {
        let dir = TempDir::new();
        let cfg = Config::load_project(&dir.0).unwrap();
        assert_eq!(cfg.staged, None);
        assert_eq!(cfg.highlight, None);
    }

    #[test]
    fn malformed_config_returns_error() {
        let (dir, _path) = write_tmp_config("this is = = not valid toml {{{\n");
        let err = Config::load_project(&dir.0).unwrap_err();
        assert!(
            err.contains("invalid config"),
            "malformed TOML should fail loudly, got: {err}"
        );
    }

    #[test]
    fn illegal_layout_returns_error() {
        let (dir, _path) = write_tmp_config("layout = \"sidebyside\"\n");
        let err = Config::load_project(&dir.0).unwrap_err();
        assert!(
            err.contains("layout") && err.contains("unified"),
            "illegal layout must name field + allowed values, got: {err}"
        );
    }

    #[test]
    fn illegal_scope_returns_error() {
        let (dir, _path) = write_tmp_config("scope = \"everything\"\n");
        let err = Config::load_project(&dir.0).unwrap_err();
        assert!(
            err.contains("scope") && err.contains("worktree"),
            "illegal scope must name field + allowed values, got: {err}"
        );
    }

    #[test]
    fn illegal_vcs_returns_error() {
        let (dir, _path) = write_tmp_config("vcs = \"fossil\"\n");
        let err = Config::load_project(&dir.0).unwrap_err();
        assert!(
            err.contains("vcs") && err.contains("auto"),
            "illegal vcs must name field + allowed values, got: {err}"
        );
    }

    #[test]
    fn load_layers_user_and_project() {
        // This test mutates the process-global $HOME / $XDG_CONFIG_HOME, which
        // races with any other test (here or in cli_parse) that reads those
        // vars under parallel execution. Serialize on a shared mutex so the
        // environment is consistent for the duration of the test.
        static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_MUTEX.lock().unwrap();
        // We can only realistically test the project layer here (user path
        // depends on $HOME/$XDG). But load() == user.merge(project); with no
        // user config present it should equal the project config.
        let (dir, _path) = write_tmp_config("highlight = false\nwatch = true\n");
        // Point HOME at an empty dir so load_user() finds nothing.
        let empty_home = TempDir::new();
        let prev_home = std::env::var_os("HOME");
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("HOME", &empty_home.0);
        std::env::remove_var("XDG_CONFIG_HOME");

        let cfg = Config::load(&dir.0).unwrap();
        assert_eq!(cfg.highlight, Some(false));
        assert_eq!(cfg.watch, Some(true));

        // restore env
        if let Some(h) = prev_home {
            std::env::set_var("HOME", h);
        }
        if let Some(x) = prev_xdg {
            std::env::set_var("XDG_CONFIG_HOME", x);
        }
    }

    #[test]
    fn layout_mode_defaults_to_unified() {
        let cfg = Config::default();
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert_eq!(r.layout, LayoutMode::Unified);
    }

    #[test]
    fn layout_mode_from_config() {
        let (dir, _path) = write_tmp_config("layout = \"stack\"\n");
        let cfg = Config::load_project(&dir.0).unwrap();
        assert_eq!(cfg.layout.as_deref(), Some("stack"));
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert_eq!(r.layout, LayoutMode::Stack);
    }

    #[test]
    fn layout_mode_from_str() {
        assert_eq!(LayoutMode::parse_str("unified"), LayoutMode::Unified);
        assert_eq!(LayoutMode::parse_str("stack"), LayoutMode::Stack);
        assert_eq!(LayoutMode::parse_str("split"), LayoutMode::Split);
        assert_eq!(LayoutMode::parse_str("SPLIT"), LayoutMode::Split);
        assert_eq!(LayoutMode::parse_str("unknown"), LayoutMode::Unified);
        assert_eq!(LayoutMode::parse_str(""), LayoutMode::Unified);
        assert!(LayoutMode::try_parse("sidebyside").is_err());
        assert!(LayoutMode::try_parse("stack").is_ok());
    }

    #[test]
    fn layout_mode_as_str() {
        assert_eq!(LayoutMode::Unified.as_str(), "unified");
        assert_eq!(LayoutMode::Stack.as_str(), "stack");
        assert_eq!(LayoutMode::Split.as_str(), "split");
    }

    #[test]
    fn layout_mode_split_from_config() {
        let (dir, _path) = write_tmp_config("layout = \"split\"\n");
        let cfg = Config::load_project(&dir.0).unwrap();
        assert_eq!(cfg.layout.as_deref(), Some("split"));
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert_eq!(r.layout, LayoutMode::Split);
    }

    #[test]
    fn layout_mode_cli_overrides_config() {
        let (dir, _path) = write_tmp_config("layout = \"stack\"\n");
        let cfg = Config::load_project(&dir.0).unwrap();
        let cli = CliFlags {
            layout: Some(LayoutMode::Split),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert_eq!(r.layout, LayoutMode::Split);
    }

    #[test]
    fn wrap_defaults_to_false() {
        let cfg = Config::default();
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert!(!r.wrap, "wrap should default to false");
    }

    #[test]
    fn wrap_from_config() {
        let (dir, _path) = write_tmp_config("wrap = true\n");
        let cfg = Config::load_project(&dir.0).unwrap();
        assert_eq!(cfg.wrap, Some(true));
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert!(r.wrap);
    }

    #[test]
    fn wrap_false_from_config() {
        let (dir, _path) = write_tmp_config("wrap = false\n");
        let cfg = Config::load_project(&dir.0).unwrap();
        assert_eq!(cfg.wrap, Some(false));
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert!(!r.wrap);
    }

    #[test]
    fn export_on_quit_defaults_to_none() {
        let cfg = Config::default();
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert_eq!(r.export_on_quit, ExportOnQuit::None);
    }

    #[test]
    fn export_on_quit_from_config_and_cli() {
        let cfg = Config {
            export_on_quit: Some("json".into()),
            ..Default::default()
        };
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert_eq!(r.export_on_quit, ExportOnQuit::Json);

        let cli = CliFlags {
            export_on_quit: Some(ExportOnQuit::Both),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert_eq!(r.export_on_quit, ExportOnQuit::Both); // CLI wins
    }

    #[test]
    fn export_on_quit_parse_str() {
        assert_eq!(ExportOnQuit::parse_str("markdown"), ExportOnQuit::Markdown);
        assert_eq!(ExportOnQuit::parse_str("md"), ExportOnQuit::Markdown);
        assert_eq!(ExportOnQuit::parse_str("weird"), ExportOnQuit::None);
        assert!(ExportOnQuit::try_parse("weird").is_err());
        assert!(ExportOnQuit::try_parse("none").is_ok());
    }

    #[test]
    fn vcs_preference_parse_str() {
        assert_eq!(VcsPreference::parse_str("auto"), VcsPreference::Auto);
        assert_eq!(VcsPreference::parse_str("git"), VcsPreference::Git);
        assert_eq!(VcsPreference::parse_str("jj"), VcsPreference::Jj);
        assert_eq!(VcsPreference::parse_str("jujutsu"), VcsPreference::Jj);
        assert_eq!(VcsPreference::parse_str("weird"), VcsPreference::Auto);
    }

    #[test]
    fn vcs_from_config_and_cli() {
        let cfg = Config {
            vcs: Some("jj".into()),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &CliFlags::default()).unwrap();
        assert_eq!(r.vcs, VcsPreference::Jj);

        let cli = CliFlags {
            vcs: Some(VcsPreference::Git),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert_eq!(r.vcs, VcsPreference::Git); // CLI wins
    }

    #[test]
    fn vcs_defaults_to_auto() {
        let r = ResolvedConfig::resolve(&Config::default(), &CliFlags::default()).unwrap();
        assert_eq!(r.vcs, VcsPreference::Auto);
    }

    #[test]
    fn persist_review_defaults_to_true() {
        let cfg = Config::default();
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert!(r.persist_review);
    }

    #[test]
    fn persist_review_cli_no_persist_disables() {
        let cfg = Config {
            persist_review: Some(true),
            ..Default::default()
        };
        let cli = CliFlags {
            persist_review: Some(false),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &cli).unwrap();
        assert!(!r.persist_review);
    }

    #[test]
    fn persist_review_from_config() {
        let (dir, _path) = write_tmp_config("persist_review = false\n");
        let cfg = Config::load_project(&dir.0).unwrap();
        assert_eq!(cfg.persist_review, Some(false));
        let r = ResolvedConfig::resolve(&cfg, &CliFlags::default()).unwrap();
        assert!(!r.persist_review);
    }
}
