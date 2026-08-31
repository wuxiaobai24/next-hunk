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

/// Layout mode for the diff stream pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutMode {
    /// Unified diff: interleaved context/add/delete per hunk (traditional).
    #[default]
    Unified,
    /// Stacked diff: old content (context + deletes) then new content
    /// (context + adds) per file, vertically.
    Stack,
    /// Side-by-side split: old and new content in two aligned columns.
    Split,
    /// Responsive: pick split / stack / unified from the live stream-pane
    /// width at draw time (≥ [`AUTO_SPLIT_MIN_WIDTH`] → split,
    /// ≥ [`AUTO_STACK_MIN_WIDTH`] → stack, else unified).
    Auto,
}

/// Width (columns) of the stream pane at which `layout = "auto"` upgrades
/// from stack to side-by-side split.
pub const AUTO_SPLIT_MIN_WIDTH: u16 = 120;
/// Width (columns) of the stream pane at which `layout = "auto"` upgrades
/// from unified to stack.
pub const AUTO_STACK_MIN_WIDTH: u16 = 40;

impl LayoutMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            LayoutMode::Unified => "unified",
            LayoutMode::Stack => "stack",
            LayoutMode::Split => "split",
            LayoutMode::Auto => "auto",
        }
    }

    /// Resolve `Auto` against a stream-pane width; concrete modes pass
    /// through unchanged.
    pub fn resolve(&self, stream_width: u16) -> LayoutMode {
        match self {
            LayoutMode::Auto => {
                if stream_width >= AUTO_SPLIT_MIN_WIDTH {
                    LayoutMode::Split
                } else if stream_width >= AUTO_STACK_MIN_WIDTH {
                    LayoutMode::Stack
                } else {
                    LayoutMode::Unified
                }
            }
            concrete => *concrete,
        }
    }

    pub fn parse_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "stack" => LayoutMode::Stack,
            "split" => LayoutMode::Split,
            "auto" => LayoutMode::Auto,
            _ => LayoutMode::Unified,
        }
    }
}

/// What a review TUI emits when the human quits (`export_on_quit` config,
/// `--export` CLI). The emitted report is a compatible extension of the
/// `--select` decisions JSON: the same three decision buckets plus the
/// session's comments and banner notes — the structured feedback loop
/// back to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportOnQuit {
    /// Emit nothing. `--select` still prints the legacy decisions-only JSON.
    #[default]
    None,
    /// One JSON object on stdout (or a `.json` file with `--export-file`).
    Json,
    /// A Markdown report (or a `.md` file with `--export-file`).
    Markdown,
    /// Both formats: JSON then Markdown on stdout, or sibling files.
    Both,
}

impl ExportOnQuit {
    /// Parse a config/CLI value. Unknown tokens fall back to [`ExportOnQuit::None`]
    /// (config); the CLI validates its own values before calling this.
    pub fn parse_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "json" => ExportOnQuit::Json,
            "markdown" | "md" => ExportOnQuit::Markdown,
            "both" => ExportOnQuit::Both,
            _ => ExportOnQuit::None,
        }
    }

    /// Parse a `--export` CLI value, rejecting unknown tokens (a typo on the
    /// command line should fail fast, not silently disable the export).
    pub fn parse_cli(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "none" => Some(ExportOnQuit::None),
            "json" => Some(ExportOnQuit::Json),
            "markdown" | "md" => Some(ExportOnQuit::Markdown),
            "both" => Some(ExportOnQuit::Both),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ExportOnQuit::None => "none",
            ExportOnQuit::Json => "json",
            ExportOnQuit::Markdown => "markdown",
            ExportOnQuit::Both => "both",
        }
    }
}

/// Raw user-configurable options. Every field optional: `None` = "not set".
/// Default `context_collapse` threshold: runs/gaps of ≥ this many unchanged
/// lines collapse to a `··· N unchanged lines ···` marker row.
pub const DEFAULT_CONTEXT_COLLAPSE: usize = 8;
/// Render width of one tab stop (in columns) when expanding tabs in diff
/// lines. Terminal-native tab stops (usually 8) break column alignment in
/// the split layout, so tabs are expanded at render time instead.
pub const DEFAULT_TAB_WIDTH: u32 = 4;
/// Upper bound for `tab_width` (mirrors hunk's 1–16 range; 0 is invalid).
pub const MAX_TAB_WIDTH: u32 = 16;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    pub staged: Option<bool>,
    pub highlight: Option<bool>,
    pub watch: Option<bool>,
    /// Show a line-number gutter column.
    pub line_numbers: Option<bool>,
    /// Include untracked files in worktree diff.
    pub include_untracked: Option<bool>,
    /// TUI theme name: "dark" / "light" / "auto" (auto = detect via $COLORFGBG).
    pub theme: Option<String>,
    /// Layout mode: "unified" (default) or "stack".
    pub layout: Option<String>,
    /// Wrap long lines in the diff stream pane. `false` = truncate (default).
    pub wrap: Option<bool>,
    /// Collapse unchanged context: runs/gaps of ≥ this many lines render as
    /// one `··· N unchanged lines ···` marker row. `0` disables. Default 8.
    pub context_collapse: Option<usize>,
    /// Center the row a navigation jump lands on (`]h`, search, file jumps)
    /// in the viewport instead of pinning it to the top. Default true.
    pub jump_center: Option<bool>,
    /// Show the review cursor row ("row", default) or hide it ("off").
    pub cursor_line: Option<String>,
    /// Tab-stop width (columns) for rendering tabs in diff lines, 1–16.
    /// Default 4. (`--tab-width` overrides.)
    pub tab_width: Option<u32>,
    /// Show the file rail (left sidebar) at startup. Accepts `true`/`false`
    /// or hunk-style `"auto"` (treated as `true`; the rail already adapts its
    /// width to the terminal). `b` toggles at runtime.
    pub sidebar: Option<toml::Value>,
    /// Render agent/human notes (💬 rows, inline annotations, rail badges).
    /// `false` = plain diff, no notes. Default `true`.
    pub agent_notes: Option<bool>,
    /// What the review TUI emits on quit: `"none"` (default), `"json"`,
    /// `"markdown"`/`"md"`, or `"both"`. Honored by `diff` and `serve`;
    /// the `--export` flag overrides it there.
    pub export_on_quit: Option<String>,
    /// Keybinding overrides: action name → key list (or `false` to unbind).
    /// See `tui::keymap` for the spec grammar. The whole table replaces the
    /// lower layer's (per-action merge is not meaningful for key claims).
    pub keybindings: Option<std::collections::HashMap<String, toml::Value>>,
}

impl Config {
    /// Merge `other` into `self`, with `other` (the higher-precedence layer)
    /// winning wherever it is `Some`. Returns `self` for chaining.
    pub fn merge(mut self, other: Config) -> Config {
        if other.staged.is_some() {
            self.staged = other.staged;
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
        if other.context_collapse.is_some() {
            self.context_collapse = other.context_collapse;
        }
        if other.cursor_line.is_some() {
            self.cursor_line = other.cursor_line;
        }
        if other.jump_center.is_some() {
            self.jump_center = other.jump_center;
        }
        if other.tab_width.is_some() {
            self.tab_width = other.tab_width;
        }
        if other.sidebar.is_some() {
            self.sidebar = other.sidebar;
        }
        if other.agent_notes.is_some() {
            self.agent_notes = other.agent_notes;
        }
        if other.export_on_quit.is_some() {
            self.export_on_quit = other.export_on_quit;
        }
        if other.keybindings.is_some() {
            self.keybindings = other.keybindings;
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
    pub staged: bool,
    pub highlight: bool,
    pub watch: bool,
    /// Show a line-number gutter column. ON by default.
    pub line_numbers: bool,
    /// Include untracked files in worktree diff. OFF by default (safe).
    pub include_untracked: bool,
    /// TUI theme name ("dark" / "light" / "auto"). `None` = use the app default
    /// (dark). Config-only in this pass — no CLI flag yet.
    pub theme: Option<String>,
    /// Layout mode for the diff stream: "unified" (default) or "stack".
    pub layout: LayoutMode,
    /// Wrap long lines in the diff stream pane. `false` = truncate (default).
    pub wrap: bool,
    /// Collapse unchanged context threshold (0 = off). See
    /// [`DEFAULT_CONTEXT_COLLAPSE`].
    pub context_collapse: usize,
    /// Show the review cursor row (`c` composes a note on it). ON by default;
    /// `cursor_line = "off"` hides the highlight (navigation still works).
    pub cursor_line: bool,
    /// Center the row a navigation jump lands on. ON by default;
    /// `jump_center = false` pins jumps to the viewport top.
    pub jump_center: bool,
    /// Tab-stop width (columns) for render-time tab expansion, 1–16.
    pub tab_width: u32,
    /// Show the file rail (left sidebar) at startup. `b` toggles at runtime.
    pub sidebar: bool,
    /// Render 💬 notes (agent + human). `false` = plain diff view.
    pub agent_notes: bool,
    /// What `diff` / `serve` emit on TUI quit. Default off (plain `--select`
    /// JSON still applies). See [`ExportOnQuit`].
    pub export_on_quit: ExportOnQuit,
    /// Resolved keybindings (defaults when no `[keybindings]` table).
    pub keymap: crate::tui::keymap::Keymap,
}

impl Default for ResolvedConfig {
    fn default() -> Self {
        // Defaults: highlight on (matches existing TUI behavior), staged/watch off.
        Self {
            staged: false,
            highlight: true,
            watch: false,
            line_numbers: true,
            include_untracked: false,
            theme: None,
            layout: LayoutMode::Unified,
            wrap: false,
            context_collapse: DEFAULT_CONTEXT_COLLAPSE,
            cursor_line: true,
            jump_center: true,
            tab_width: DEFAULT_TAB_WIDTH,
            sidebar: true,
            agent_notes: true,
            export_on_quit: ExportOnQuit::None,
            keymap: crate::tui::keymap::Keymap::default_map(),
        }
    }
}

/// Interpret a `sidebar` config value. hunk accepts `true`/`false`/`"auto"`;
/// we take booleans plus that string (as `true` — our rail is width-adaptive,
/// so "auto" needs no special case). Anything else is `None` (invalid).
fn coerce_sidebar(v: &toml::Value) -> Option<bool> {
    match v {
        toml::Value::Boolean(b) => Some(*b),
        toml::Value::String(s) => match s.trim().to_lowercase().as_str() {
            "auto" | "true" | "left" => Some(true),
            "false" | "off" | "none" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Clamp a configured tab width into the valid 1–16 range (0 and garbage
/// fall back to the default rather than panicking or silently no-op'ing).
pub fn clamp_tab_width(w: Option<u32>) -> u32 {
    match w {
        Some(n) if (1..=MAX_TAB_WIDTH).contains(&n) => n,
        _ => DEFAULT_TAB_WIDTH,
    }
}

/// Everything the renderer needs at startup, in one struct — the positional
/// boolean ladder this replaces did not scale.
#[derive(Debug, Clone)]
pub struct ViewSettings {
    pub highlight: bool,
    pub line_numbers: bool,
    pub wrap: bool,
    pub context_collapse: usize,
    pub jump_center: bool,
    pub theme: Option<String>,
    pub layout: LayoutMode,
    pub cursor_line: bool,
    pub tab_width: u32,
    pub sidebar: bool,
    pub agent_notes: bool,
    pub keymap: crate::tui::keymap::Keymap,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            highlight: true,
            line_numbers: true,
            wrap: false,
            context_collapse: DEFAULT_CONTEXT_COLLAPSE,
            jump_center: true,
            theme: None,
            layout: LayoutMode::Unified,
            cursor_line: true,
            tab_width: DEFAULT_TAB_WIDTH,
            sidebar: true,
            agent_notes: true,
            keymap: crate::tui::keymap::Keymap::default_map(),
        }
    }
}

impl From<&ResolvedConfig> for ViewSettings {
    fn from(r: &ResolvedConfig) -> Self {
        Self {
            highlight: r.highlight,
            line_numbers: r.line_numbers,
            wrap: r.wrap,
            context_collapse: r.context_collapse,
            jump_center: r.jump_center,
            theme: r.theme.clone(),
            layout: r.layout,
            cursor_line: r.cursor_line,
            tab_width: r.tab_width,
            sidebar: r.sidebar,
            agent_notes: r.agent_notes,
            keymap: r.keymap.clone(),
        }
    }
}

/// Inputs from the CLI for a single toggleable option.
///
/// Most options are "default unless overridden", so a simple `Option<bool>`
/// (CLI sets `Some(false)` for `--no-flag`, `Some(true)` for `--flag`) composes
/// cleanly with the config layer. `Default` = "no CLI override" — call sites
/// that only set one flag spread over it.
#[derive(Default)]
pub struct CliFlags {
    /// `--staged` / no flag.
    pub staged: Option<bool>,
    /// `--watch` / no flag.
    pub watch: Option<bool>,
    /// `--no-highlight` → `Some(false)`; absent → `None`.
    pub highlight: Option<bool>,
    /// `--include-untracked` → `Some(true)`; absent → `None`.
    pub include_untracked: Option<bool>,
    /// `--tab-width <N>`; absent → `None`.
    pub tab_width: Option<u32>,
}

/// Collect warnings for stringly-typed config values that will silently fall
/// back to a default below — a typo like `layout = "spilt"` otherwise quietly
/// changes behavior. Keybinding overrides already warn; this extends the same
/// courtesy to the rest of the config surface. The caller prints before the
/// TUI takes over the screen, so stderr stays visible.
fn config_warnings(cfg: &Config, cli_tab_width: Option<u32>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(l) = cfg.layout.as_deref() {
        let l = l.trim().to_lowercase();
        if !matches!(l.as_str(), "unified" | "stack" | "split" | "auto") {
            out.push(format!(
                "unknown layout {l:?} (valid: unified, stack, split, auto) — using unified"
            ));
        }
    }
    if let Some(e) = cfg.export_on_quit.as_deref() {
        let e = e.trim().to_lowercase();
        if !matches!(e.as_str(), "none" | "json" | "markdown" | "md" | "both") {
            out.push(format!(
                "unknown export_on_quit {e:?} (valid: none, json, markdown, both) — using none"
            ));
        }
    }
    if let Some(v) = &cfg.sidebar {
        if coerce_sidebar(v).is_none() {
            out.push(format!(
                "invalid sidebar value {v:?} (valid: true, false, \"auto\") — using the default"
            ));
        }
    }
    if let Some(v) = cfg.cursor_line.as_deref() {
        let v = v.trim().to_lowercase();
        if !matches!(v.as_str(), "on" | "true" | "off" | "false") {
            out.push(format!(
                "invalid cursor_line {v:?} (valid: on/true, off/false) — treating as on"
            ));
        }
    }
    let tab = cli_tab_width.or(cfg.tab_width);
    if let Some(n) = tab {
        if !(1..=MAX_TAB_WIDTH).contains(&n) {
            out.push(format!(
                "tab_width {n} out of range (1–{MAX_TAB_WIDTH}) — using {DEFAULT_TAB_WIDTH}"
            ));
        }
    }
    out
}

impl ResolvedConfig {
    /// Resolve the final config.
    ///
    /// CLI `Some` wins; otherwise the merged config; otherwise defaults.
    pub fn resolve(cfg: &Config, cli: &CliFlags) -> Self {
        for w in config_warnings(cfg, cli.tab_width) {
            eprintln!("warning: {w}");
        }
        let d = Self::default();
        Self {
            staged: cli.staged.or(cfg.staged).unwrap_or(d.staged),
            highlight: cli.highlight.or(cfg.highlight).unwrap_or(d.highlight),
            watch: cli.watch.or(cfg.watch).unwrap_or(d.watch),
            line_numbers: cfg.line_numbers.unwrap_or(d.line_numbers),
            include_untracked: cli
                .include_untracked
                .or(cfg.include_untracked)
                .unwrap_or(d.include_untracked),
            theme: cfg.theme.clone(),
            layout: cfg
                .layout
                .as_deref()
                .map(LayoutMode::parse_str)
                .unwrap_or(d.layout),
            wrap: cfg.wrap.unwrap_or(d.wrap),
            context_collapse: cfg.context_collapse.unwrap_or(d.context_collapse),
            cursor_line: cfg
                .cursor_line
                .as_deref()
                .map(|v| {
                    let v = v.trim().to_lowercase();
                    v != "off" && v != "false"
                })
                .unwrap_or(d.cursor_line),
            jump_center: cfg.jump_center.unwrap_or(d.jump_center),
            tab_width: clamp_tab_width(cli.tab_width.or(cfg.tab_width)),
            sidebar: cfg
                .sidebar
                .as_ref()
                .and_then(coerce_sidebar)
                .unwrap_or(d.sidebar),
            agent_notes: cfg.agent_notes.unwrap_or(d.agent_notes),
            export_on_quit: cfg
                .export_on_quit
                .as_deref()
                .map(ExportOnQuit::parse_str)
                .unwrap_or(d.export_on_quit),
            keymap: match &cfg.keybindings {
                Some(overrides) => {
                    let (km, warnings) = crate::tui::keymap::Keymap::with_overrides(overrides);
                    // Resolve runs before the TUI takes over the screen, so
                    // stderr is still visible to the user.
                    for w in warnings {
                        eprintln!("warning: {w}");
                    }
                    km
                }
                None => d.keymap,
            },
        }
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
            use std::sync::atomic::{AtomicU64, Ordering};
            // pid + a process-unique counter. The timestamp alone is not
            // unique: macOS SystemTime::now() has ~1µs granularity, so two
            // parallel tests can stamp the same "nanos", share a directory,
            // and one's Drop deletes the other's config mid-test.
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "next-hunk-cfg-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
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

    fn test_flags() -> CliFlags {
        CliFlags {
            staged: None,
            watch: None,
            highlight: None,
            include_untracked: None,
            tab_width: None,
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
    fn config_warnings_flag_typos_and_range_violations() {
        let cfg = Config {
            layout: Some("spilt".into()),
            export_on_quit: Some("jsoon".into()),
            sidebar: Some(toml::Value::String("leftish".into())),
            cursor_line: Some("of".into()),
            tab_width: Some(99),
            ..Config::default()
        };
        let ws = config_warnings(&cfg, None);
        assert_eq!(ws.len(), 5, "{ws:?}");
        assert!(ws[0].contains("layout"), "{ws:?}");
        assert!(ws[1].contains("export_on_quit"), "{ws:?}");
        assert!(ws[2].contains("sidebar"), "{ws:?}");
        assert!(ws[3].contains("cursor_line"), "{ws:?}");
        assert!(ws[4].contains("tab_width"), "{ws:?}");

        // Valid values (any case) produce no warnings; CLI tab width counts.
        let ok = Config {
            layout: Some("Split".into()),
            export_on_quit: Some("MD".into()),
            sidebar: Some(toml::Value::String("Auto".into())),
            cursor_line: Some("Off".into()),
            tab_width: Some(8),
            ..Config::default()
        };
        assert!(config_warnings(&ok, None).is_empty());
        assert_eq!(
            config_warnings(&Config::default(), Some(99)).len(),
            1,
            "out-of-range CLI tab width warns"
        );
        assert!(config_warnings(&Config::default(), Some(4)).is_empty());
    }

    #[test]
    fn cursor_line_parse_is_case_insensitive() {
        // "Off" used to be treated as on (case-sensitive compare).
        let cfg = Config {
            cursor_line: Some("Off".into()),
            ..Config::default()
        };
        assert!(!ResolvedConfig::resolve(&cfg, &CliFlags::default()).cursor_line);
    }

    #[test]
    fn resolve_cli_overrides_config() {
        let cfg = Config {
            staged: Some(true),
            highlight: Some(false),
            watch: Some(true),
            ..Default::default()
        };
        let cli = CliFlags {
            staged: Some(false), // CLI overrides
            ..test_flags()
        };
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(!r.staged); // CLI wins
        assert!(!r.highlight); // config wins (CLI None)
        assert!(r.watch); // config wins
    }

    #[test]
    fn resolve_defaults_when_nothing_set() {
        let cfg = Config::default();
        let cli = test_flags();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(!r.staged);
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
            ..test_flags()
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
        let cli = test_flags();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(!r.line_numbers); // config false wins
    }

    #[test]
    fn resolve_cursor_line_defaults_to_on_and_off_disables() {
        let cli = test_flags();
        let r = ResolvedConfig::resolve(&Config::default(), &cli);
        assert!(r.cursor_line, "cursor row highlight defaults on");

        let cfg = Config {
            cursor_line: Some("off".into()),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(!r.cursor_line, "cursor_line = \"off\" hides the highlight");

        let cfg = Config {
            cursor_line: Some("row".into()),
            ..Default::default()
        };
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(r.cursor_line);
    }

    #[test]
    fn resolve_line_numbers_defaults_to_true() {
        let cfg = Config::default(); // line_numbers = None
        let cli = test_flags();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(r.line_numbers); // default on
    }

    #[test]
    fn resolve_carries_theme_from_config() {
        let cfg = Config {
            theme: Some("light".into()),
            ..Default::default()
        };
        let cli = test_flags();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert_eq!(r.theme.as_deref(), Some("light"));
    }

    #[test]
    fn resolve_theme_none_when_unset() {
        let cfg = Config::default();
        let cli = test_flags();
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
        let cli = test_flags();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert_eq!(r.layout, LayoutMode::Unified);
    }

    #[test]
    fn layout_mode_from_config() {
        let (dir, _path) = write_tmp_config("layout = \"stack\"\n");
        let cfg = Config::load_project(&dir.0);
        assert_eq!(cfg.layout.as_deref(), Some("stack"));
        let cli = test_flags();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert_eq!(r.layout, LayoutMode::Stack);
    }

    #[test]
    fn layout_mode_from_str() {
        assert_eq!(LayoutMode::parse_str("unified"), LayoutMode::Unified);
        assert_eq!(LayoutMode::parse_str("stack"), LayoutMode::Stack);
        assert_eq!(LayoutMode::parse_str("unknown"), LayoutMode::Unified);
        assert_eq!(LayoutMode::parse_str(""), LayoutMode::Unified);
    }

    #[test]
    fn layout_mode_as_str() {
        assert_eq!(LayoutMode::Unified.as_str(), "unified");
        assert_eq!(LayoutMode::Stack.as_str(), "stack");
    }

    #[test]
    fn wrap_defaults_to_false() {
        let cfg = Config::default();
        let cli = test_flags();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(!r.wrap, "wrap should default to false");
    }

    #[test]
    fn wrap_from_config() {
        let (dir, _path) = write_tmp_config("wrap = true\n");
        let cfg = Config::load_project(&dir.0);
        assert_eq!(cfg.wrap, Some(true));
        let cli = test_flags();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(r.wrap);
    }

    #[test]
    fn wrap_false_from_config() {
        let (dir, _path) = write_tmp_config("wrap = false\n");
        let cfg = Config::load_project(&dir.0);
        assert_eq!(cfg.wrap, Some(false));
        let cli = test_flags();
        let r = ResolvedConfig::resolve(&cfg, &cli);
        assert!(!r.wrap);
    }

    #[test]
    fn tab_width_defaults_and_clamps() {
        let r = ResolvedConfig::resolve(&Config::default(), &test_flags());
        assert_eq!(r.tab_width, DEFAULT_TAB_WIDTH);

        // in-range values pass through from config and CLI (CLI wins)
        let (dir, _path) = write_tmp_config("tab_width = 2\n");
        let cfg = Config::load_project(&dir.0);
        let r = ResolvedConfig::resolve(&cfg, &test_flags());
        assert_eq!(r.tab_width, 2);
        let r = ResolvedConfig::resolve(
            &cfg,
            &CliFlags {
                tab_width: Some(8),
                ..test_flags()
            },
        );
        assert_eq!(r.tab_width, 8);

        // out-of-range falls back to the default, never panics / no-ops
        for bad in [0, 17, 999] {
            let (dir, _path) = write_tmp_config(&format!("tab_width = {bad}\n"));
            let cfg = Config::load_project(&dir.0);
            let r = ResolvedConfig::resolve(&cfg, &test_flags());
            assert_eq!(r.tab_width, DEFAULT_TAB_WIDTH, "tab_width = {bad}");
        }
    }

    #[test]
    fn sidebar_accepts_bool_and_hunk_strings() {
        for (toml_val, expect) in [("true", true), ("false", false), ("\"auto\"", true)] {
            let (dir, _path) = write_tmp_config(&format!("sidebar = {toml_val}\n"));
            let cfg = Config::load_project(&dir.0);
            let r = ResolvedConfig::resolve(&cfg, &test_flags());
            assert_eq!(r.sidebar, expect, "sidebar = {toml_val}");
        }
        // default: shown
        let r = ResolvedConfig::resolve(&Config::default(), &test_flags());
        assert!(r.sidebar);
        // invalid type/string falls back to the default (not a crash)
        let (dir, _path) = write_tmp_config("sidebar = \" sideways\"\n");
        let cfg = Config::load_project(&dir.0);
        let r = ResolvedConfig::resolve(&cfg, &test_flags());
        assert!(r.sidebar);
    }

    #[test]
    fn export_on_quit_resolves_from_config() {
        use ExportOnQuit as E;
        // default: off
        let r = ResolvedConfig::resolve(&Config::default(), &test_flags());
        assert_eq!(r.export_on_quit, E::None);
        for (toml_val, expect) in [
            ("\"json\"", E::Json),
            ("\"markdown\"", E::Markdown),
            ("\"md\"", E::Markdown),
            ("\"both\"", E::Both),
            ("\"none\"", E::None),
        ] {
            let (dir, _path) = write_tmp_config(&format!("export_on_quit = {toml_val}\n"));
            let cfg = Config::load_project(&dir.0);
            let r = ResolvedConfig::resolve(&cfg, &test_flags());
            assert_eq!(r.export_on_quit, expect, "export_on_quit = {toml_val}");
        }
        // unknown value falls back to off (not a crash, not a surprise format)
        let (dir, _path) = write_tmp_config("export_on_quit = \"yaml\"\n");
        let cfg = Config::load_project(&dir.0);
        let r = ResolvedConfig::resolve(&cfg, &test_flags());
        assert_eq!(r.export_on_quit, E::None);
    }

    #[test]
    fn agent_notes_default_true_and_configurable() {
        let r = ResolvedConfig::resolve(&Config::default(), &test_flags());
        assert!(r.agent_notes);
        let (dir, _path) = write_tmp_config("agent_notes = false\n");
        let cfg = Config::load_project(&dir.0);
        let r = ResolvedConfig::resolve(&cfg, &test_flags());
        assert!(!r.agent_notes);
    }

    #[test]
    fn keybindings_table_parses_and_resolves() {
        let (dir, _path) =
            write_tmp_config("[keybindings]\nquit = [\"Q\", \"ctrl-x\"]\nhelp = false\n");
        let cfg = Config::load_project(&dir.0);
        let kb = cfg.keybindings.clone().expect("keybindings table parsed");
        assert_eq!(kb.len(), 2);
        let r = ResolvedConfig::resolve(&cfg, &test_flags());
        // resolved keymap reflects the overrides
        assert_eq!(
            r.keymap.lookup(&crossterm_key('Q')),
            Some(crate::tui::keymap::Action::Quit)
        );
        assert_eq!(
            r.keymap.lookup(&crossterm_key('q')),
            None,
            "default q unbound by the override"
        );
        assert!(
            r.keymap
                .keys_for(crate::tui::keymap::Action::Help)
                .is_empty(),
            "help unbound"
        );
    }

    #[test]
    fn malformed_keybinding_values_warn_not_crash() {
        let (dir, _path) = write_tmp_config(
            "[keybindings]\nquit = [12345]\nteleport = \"x\"\ncursor_down = \"junk-key\"\n",
        );
        let cfg = Config::load_project(&dir.0);
        let r = ResolvedConfig::resolve(&cfg, &test_flags());
        // junk spec ignored, quit keeps its defaults
        assert_eq!(
            r.keymap.lookup(&crossterm_key('q')),
            Some(crate::tui::keymap::Action::Quit)
        );
    }

    fn crossterm_key(c: char) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(c),
            crossterm::event::KeyModifiers::NONE,
        )
    }

    #[test]
    fn view_settings_from_resolved_round_trip() {
        let r = ResolvedConfig::resolve(
            &Config {
                layout: Some("split".into()),
                wrap: Some(true),
                tab_width: Some(2),
                sidebar: Some(toml::Value::Boolean(false)),
                agent_notes: Some(false),
                ..Default::default()
            },
            &test_flags(),
        );
        let v = ViewSettings::from(&r);
        assert_eq!(v.layout, LayoutMode::Split);
        assert!(v.wrap);
        assert_eq!(v.tab_width, 2);
        assert!(!v.sidebar);
        assert!(!v.agent_notes);
        assert!(v.highlight);
        assert!(v.line_numbers);
        assert!(v.cursor_line);
    }
}
