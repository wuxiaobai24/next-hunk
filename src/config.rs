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

    /// Parse config values. Unknown → `Worktree` (safe default).
    pub fn parse_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "staged" | "cached" | "index" => DiffScope::Staged,
            "working-set" | "working_set" | "all" | "ws" => DiffScope::WorkingSet,
            "worktree" | "unstaged" | "wt" => DiffScope::Worktree,
            _ => DiffScope::Worktree,
        }
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

    /// Parse config/CLI values. Unknown → `None` (safe default).
    pub fn parse_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "json" => ExportOnQuit::Json,
            "markdown" | "md" => ExportOnQuit::Markdown,
            "both" => ExportOnQuit::Both,
            _ => ExportOnQuit::None,
        }
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

    pub fn parse_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "stack" => LayoutMode::Stack,
            "split" => LayoutMode::Split,
            _ => LayoutMode::Unified,
        }
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
    /// When a live `serve` exists for this repo, `diff --focus` / `--note`
    /// forwards as `push` instead of opening a second TUI. Default: true.
    /// Set `false` (or pass `--no-forward`) to always open a one-shot TUI.
    pub auto_forward: Option<bool>,
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
        if other.auto_forward.is_some() {
            self.auto_forward = other.auto_forward;
        }
        self
    }

    /// Load the user-level config from `~/.config/next-hunk/config.toml`
    /// (honoring `$XDG_CONFIG_HOME` and `$HOME`). Missing file = empty config.
    pub fn load_user() -> Config {
        user_config_path()
            .and_then(|p| load_file(&p))
            .unwrap_or_default()
    }

    /// Load the project-level config by walking up from `start` looking for a
    /// `.next-hunk/config.toml`. Missing = empty config.
    pub fn load_project(start: &Path) -> Config {
        find_project_config(start)
            .and_then(|p| load_file(&p))
            .unwrap_or_default()
    }

    /// Load the full layered config: user merged with project (project wins).
    pub fn load(start: &Path) -> Config {
        Config::load_user().merge(Config::load_project(start))
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
    /// When true (default), `diff --focus`/`--note` forwards into a live serve.
    pub auto_forward: bool,
}

impl Default for ResolvedConfig {
    fn default() -> Self {
        // Defaults: highlight on (matches existing TUI behavior), worktree/watch off.
        // auto_forward on so agents need not list/push when a human already has serve.
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
            auto_forward: true,
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
    /// `--no-forward` → `Some(false)`; absent → `None` (use config/default).
    pub auto_forward: Option<bool>,
}

impl ResolvedConfig {
    /// Resolve the final config.
    ///
    /// CLI `Some` wins; otherwise the merged config; otherwise defaults.
    pub fn resolve(cfg: &Config, cli: &CliFlags) -> Self {
        let d = Self::default();
        Self {
            scope: resolve_scope(cfg, cli),
            highlight: cli.highlight.or(cfg.highlight).unwrap_or(d.highlight),
            watch: cli.watch.or(cfg.watch).unwrap_or(d.watch),
            line_numbers: cfg.line_numbers.unwrap_or(d.line_numbers),
            include_untracked: cli
                .include_untracked
                .or(cfg.include_untracked)
                .unwrap_or(d.include_untracked),
            theme: cfg.theme.clone(),
            layout: cli
                .layout
                .or_else(|| cfg.layout.as_deref().map(LayoutMode::parse_str))
                .unwrap_or(d.layout),
            wrap: cfg.wrap.unwrap_or(d.wrap),
            export_on_quit: cli
                .export_on_quit
                .or_else(|| cfg.export_on_quit.as_deref().map(ExportOnQuit::parse_str))
                .unwrap_or(d.export_on_quit),
            auto_forward: cli
                .auto_forward
                .or(cfg.auto_forward)
                .unwrap_or(d.auto_forward),
        }
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
fn resolve_scope(cfg: &Config, cli: &CliFlags) -> DiffScope {
    if cli.all == Some(true) {
        return DiffScope::WorkingSet;
    }
    if cli.staged == Some(true) {
        return DiffScope::Staged;
    }
    // Explicit CLI staged=false (shouldn't happen with a pure flag) keeps config.
    if let Some(scope) = cfg.scope.as_deref() {
        return DiffScope::parse_str(scope);
    }
    if cfg.staged == Some(true) {
        return DiffScope::Staged;
    }
    DiffScope::Worktree
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

/// Read + parse a config file. Returns `None` on any I/O or parse error so a
/// malformed config never crashes the app.
///
/// A missing file is normal (most users have no config) and is silent. We only
/// warn when the file *exists* but can't be read or parsed — that's the case
/// worth surfacing to the user.
fn load_file(path: &Path) -> Option<Config> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No config file is the common case — stay quiet.
            return None;
        }
        Err(e) => {
            eprintln!("warning: cannot read config {}: {e}", path.display());
            return None;
        }
    };
    match toml::from_str::<Config>(&text) {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("warning: invalid config {}: {e}", path.display());
            None
        }
    }
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
        let r = ResolvedConfig::resolve(&cfg, &cli);
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
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert_eq!(r.scope, DiffScope::Staged);
    }

    #[test]
    fn resolve_config_scope_working_set() {
        let cfg = Config {
            scope: Some("working-set".into()),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &CliFlags::default());
        assert_eq!(r.scope, DiffScope::WorkingSet);
    }

    #[test]
    fn resolve_legacy_staged_config() {
        let cfg = Config {
            staged: Some(true),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &CliFlags::default());
        assert_eq!(r.scope, DiffScope::Staged);
    }

    #[test]
    fn resolve_defaults_when_nothing_set() {
        let cfg = Config::default();
        let r = ResolvedConfig::resolve(&cfg, &CliFlags::default());
        assert_eq!(r.scope, DiffScope::Worktree);
        assert!(r.highlight); // default on
        assert!(!r.watch);
        assert!(r.line_numbers); // default on
        assert!(r.auto_forward); // default on
    }

    #[test]
    fn resolve_auto_forward_false_from_config() {
        let cfg = Config {
            auto_forward: Some(false),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &CliFlags::default());
        assert!(!r.auto_forward);
    }

    #[test]
    fn resolve_no_forward_cli_overrides_config() {
        let cfg = Config {
            auto_forward: Some(true),
            ..Default::default()
        };
        let cli = CliFlags {
            auto_forward: Some(false), // --no-forward
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(!r.auto_forward);
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
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(!r.highlight);
    }

    #[test]
    fn resolve_line_numbers_from_config() {
        let cfg = Config {
            line_numbers: Some(false),
            ..Default::default()
        };
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(!r.line_numbers); // config false wins
    }

    #[test]
    fn resolve_line_numbers_defaults_to_true() {
        let cfg = Config::default(); // line_numbers = None
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(r.line_numbers); // default on
    }

    #[test]
    fn resolve_carries_theme_from_config() {
        let cfg = Config {
            theme: Some("light".into()),
            ..Default::default()
        };
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert_eq!(r.theme.as_deref(), Some("light"));
    }

    #[test]
    fn resolve_theme_none_when_unset() {
        let cfg = Config::default();
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli);
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
        let cfg = Config::load_project(&dir.0);
        assert_eq!(cfg.staged, Some(true));
        assert_eq!(cfg.highlight, Some(false));
        assert_eq!(cfg.watch, Some(true));
        assert_eq!(cfg.line_numbers, Some(true));
        assert_eq!(cfg.theme.as_deref(), Some("dark"));
    }

    #[test]
    fn parse_partial_config() {
        let (dir, _path) = write_tmp_config("highlight = false\n");
        let cfg = Config::load_project(&dir.0);
        assert_eq!(cfg.highlight, Some(false));
        assert_eq!(cfg.staged, None); // unset
    }

    #[test]
    fn parse_unknown_field_is_ignored() {
        // Unknown keys shouldn't break parsing (forward-compat).
        let (dir, _path) = write_tmp_config("highlight = true\nfuture_field = 42\n");
        let cfg = Config::load_project(&dir.0);
        assert_eq!(cfg.highlight, Some(true));
    }

    #[test]
    fn find_project_config_walks_up() {
        let (dir, _path) = write_tmp_config("highlight = false\n");
        // create a nested subdir; config is found by walking up
        let nested = dir.0.join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        let cfg = Config::load_project(&nested);
        assert_eq!(cfg.highlight, Some(false));
    }

    #[test]
    fn missing_config_returns_empty() {
        let dir = TempDir::new();
        let cfg = Config::load_project(&dir.0);
        assert_eq!(cfg.staged, None);
        assert_eq!(cfg.highlight, None);
    }

    #[test]
    fn malformed_config_returns_empty() {
        let (dir, _path) = write_tmp_config("this is = = not valid toml {{{\n");
        let cfg = Config::load_project(&dir.0);
        // parse error → empty config (does not panic)
        assert_eq!(cfg.highlight, None);
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

        let cfg = Config::load(&dir.0);
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
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert_eq!(r.layout, LayoutMode::Unified);
    }

    #[test]
    fn layout_mode_from_config() {
        let (dir, _path) = write_tmp_config("layout = \"stack\"\n");
        let cfg = Config::load_project(&dir.0);
        assert_eq!(cfg.layout.as_deref(), Some("stack"));
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli);
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
        let cfg = Config::load_project(&dir.0);
        assert_eq!(cfg.layout.as_deref(), Some("split"));
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert_eq!(r.layout, LayoutMode::Split);
    }

    #[test]
    fn layout_mode_cli_overrides_config() {
        let (dir, _path) = write_tmp_config("layout = \"stack\"\n");
        let cfg = Config::load_project(&dir.0);
        let cli = CliFlags {
            layout: Some(LayoutMode::Split),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert_eq!(r.layout, LayoutMode::Split);
    }

    #[test]
    fn wrap_defaults_to_false() {
        let cfg = Config::default();
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(!r.wrap, "wrap should default to false");
    }

    #[test]
    fn wrap_from_config() {
        let (dir, _path) = write_tmp_config("wrap = true\n");
        let cfg = Config::load_project(&dir.0);
        assert_eq!(cfg.wrap, Some(true));
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(r.wrap);
    }

    #[test]
    fn wrap_false_from_config() {
        let (dir, _path) = write_tmp_config("wrap = false\n");
        let cfg = Config::load_project(&dir.0);
        assert_eq!(cfg.wrap, Some(false));
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(!r.wrap);
    }

    #[test]
    fn export_on_quit_defaults_to_none() {
        let cfg = Config::default();
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert_eq!(r.export_on_quit, ExportOnQuit::None);
    }

    #[test]
    fn export_on_quit_from_config_and_cli() {
        let cfg = Config {
            export_on_quit: Some("json".into()),
            ..Default::default()
        };
        let cli = CliFlags::default();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert_eq!(r.export_on_quit, ExportOnQuit::Json);

        let cli = CliFlags {
            export_on_quit: Some(ExportOnQuit::Both),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert_eq!(r.export_on_quit, ExportOnQuit::Both); // CLI wins
    }

    #[test]
    fn export_on_quit_parse_str() {
        assert_eq!(ExportOnQuit::parse_str("markdown"), ExportOnQuit::Markdown);
        assert_eq!(ExportOnQuit::parse_str("md"), ExportOnQuit::Markdown);
        assert_eq!(ExportOnQuit::parse_str("weird"), ExportOnQuit::None);
    }
}
