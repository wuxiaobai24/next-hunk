//! TUI color theme — semantic color slots, not raw [`Color`] literals.
//!
//! `view.rs` reads colors from [`Theme`] instead of hardcoding them, so the
//! chrome adapts to a light vs. dark terminal background. [`ThemeMode`] holds
//! the user's light/dark/auto choice for the **default** Flexoki palette;
//! [`ThemePreset`] selects a named chrome palette (Catppuccin, Tokyo Night, …).
//! Optional [`ThemeColorOverrides`] tweak individual slots from config.
//!
//! Syntax highlight (syntect) stays on `base16-ocean.{light,dark}` — chosen by
//! whether the active chrome is light or dark, never by inventing a per-preset
//! syntect theme.

use ratatui::style::Color;

/// Build a `Color::Rgb` from a `0xRRGGBB` literal, so palettes read as hex
/// rather than three loose bytes. `const` so it costs nothing at runtime.
const fn hex(c: u32) -> Color {
    Color::Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// A set of semantic color slots covering everything `view.rs` paints.
///
/// Colors are stored by value (small, `Copy`) so the view can read them
/// cheaply per cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// `+` line prefix on Add lines.
    pub add: Color,
    /// `-` line prefix on Delete lines.
    pub delete: Color,
    /// Word-level emphasis color for changed words on Add lines.
    pub word_add: Color,
    /// Word-level emphasis color for changed words on Delete lines.
    pub word_del: Color,
    /// Dimmed text: meta (`\`), line-number gutter, normal-mode indicator.
    pub dim: Color,
    /// File header line (`diff --git` / path).
    pub file_header: Color,
    /// Hunk header line (`@@ … @@`).
    pub hunk_header: Color,
    /// Foreground of the selected (cursor) stream row.
    pub selection_fg: Color,
    /// Background highlight of the selected stream row.
    pub selection_bg: Color,
    /// Foreground of the active search match.
    pub match_active_fg: Color,
    /// Background highlight of the active search match.
    pub match_active_bg: Color,
    /// Background of inactive (non-focused) search matches.
    pub match_inactive_bg: Color,
    /// Status-bar text color when editing a query/filter.
    pub edit_mode_fg: Color,
    /// Status-bar background.
    pub status_bg: Color,
    /// Agent annotation text (`--note` rows).
    pub note: Color,
}

impl Theme {
    /// Flexoki dark variant — tuned for a black terminal background
    /// (`#100F0F`), using the brighter 400-level accents.
    pub fn dark() -> Self {
        Self {
            add: hex(0x879A39),    // green-400
            delete: hex(0xD14D41), // red-400
            // Word-level emphasis is rendered reversed+bold, so the brighter
            // 300-level shades become solid color blocks that pop on dark.
            word_add: hex(0xA0AF54),          // green-300
            word_del: hex(0xE8705F),          // red-300
            dim: hex(0x878580),               // base-500 (gutter, meta, help line)
            file_header: hex(0xCE5D97),       // magenta-400 (bold)
            hunk_header: hex(0x4385BE),       // blue-400 (bold)
            selection_fg: hex(0xFFFCF0),      // paper
            selection_bg: hex(0x575653),      // base-700 (rail bar)
            match_active_fg: hex(0x100F0F),   // black
            match_active_bg: hex(0xDFB431),   // yellow-300 (gold match)
            match_inactive_bg: hex(0x403E3C), // base-800 (subdued)
            edit_mode_fg: hex(0xDA702C),      // orange-400 (active prompt)
            status_bg: hex(0x282726),         // base-900 (status band)
            note: hex(0x3AA99F),              // cyan-400 (italic agent notes)
        }
    }

    /// Flexoki light variant — tuned for a paper terminal background
    /// (`#FFFCF0`), using the deeper 600-level accents which stay readable on
    /// white/paper.
    pub fn light() -> Self {
        Self {
            add: hex(0x66800B),               // green-600
            delete: hex(0xAF3029),            // red-600
            word_add: hex(0x879A39),          // green-400
            word_del: hex(0xD14D41),          // red-400
            dim: hex(0x6F6E69),               // base-600 (gutter, meta, help line)
            file_header: hex(0xA02F6F),       // magenta-600 (bold)
            hunk_header: hex(0x205EA6),       // blue-600 (bold)
            selection_fg: hex(0x100F0F),      // black ink
            selection_bg: hex(0xCECDC3),      // base-200 (rail bar)
            match_active_fg: hex(0x100F0F),   // black
            match_active_bg: hex(0xDFB431),   // yellow-300 (gold match)
            match_inactive_bg: hex(0xCECDC3), // base-200 (subdued)
            edit_mode_fg: hex(0xBC5215),      // orange-600 (active prompt)
            status_bg: hex(0xE6E4D9),         // base-100 (status band)
            note: hex(0x24837B),              // cyan-600 (italic agent notes)
        }
    }

    /// [Catppuccin Mocha](https://github.com/catppuccin/catppuccin) — dark.
    pub fn catppuccin_mocha() -> Self {
        Self {
            add: hex(0xA6E3A1),               // green
            delete: hex(0xF38BA8),            // red
            word_add: hex(0x94E2D5),          // teal
            word_del: hex(0xEBA0AC),          // maroon
            dim: hex(0x6C7086),               // overlay0
            file_header: hex(0xCBA6F7),       // mauve
            hunk_header: hex(0x89B4FA),       // blue
            selection_fg: hex(0xCDD6F4),      // text
            selection_bg: hex(0x45475A),      // surface1
            match_active_fg: hex(0x1E1E2E),   // base
            match_active_bg: hex(0xF9E2AF),   // yellow
            match_inactive_bg: hex(0x313244), // surface0
            edit_mode_fg: hex(0xFAB387),      // peach
            status_bg: hex(0x181825),         // mantle
            note: hex(0x94E2D5),              // teal
        }
    }

    /// [Catppuccin Latte](https://github.com/catppuccin/catppuccin) — light.
    pub fn catppuccin_latte() -> Self {
        Self {
            add: hex(0x40A02B),               // green
            delete: hex(0xD20F39),            // red
            word_add: hex(0x179299),          // teal
            word_del: hex(0xE64553),          // maroon
            dim: hex(0x9CA0B0),               // overlay0
            file_header: hex(0x8839EF),       // mauve
            hunk_header: hex(0x1E66F5),       // blue
            selection_fg: hex(0x4C4F69),      // text
            selection_bg: hex(0xCCD0DA),      // surface0
            match_active_fg: hex(0xEFF1F5),   // base
            match_active_bg: hex(0xDF8E1D),   // yellow
            match_inactive_bg: hex(0xDCE0E8), // crust
            edit_mode_fg: hex(0xFE640B),      // peach
            status_bg: hex(0xE6E9EF),         // mantle
            note: hex(0x179299),              // teal
        }
    }

    /// [Tokyo Night](https://github.com/folke/tokyonight.nvim) storm-ish — dark.
    pub fn tokyonight() -> Self {
        Self {
            add: hex(0x9ECE6A),               // green
            delete: hex(0xF7768E),            // red
            word_add: hex(0x73DACA),          // teal-ish
            word_del: hex(0xFF9E64),          // orange
            dim: hex(0x565F89),               // comment
            file_header: hex(0xBB9AF7),       // magenta
            hunk_header: hex(0x7AA2F7),       // blue
            selection_fg: hex(0xC0CAF5),      // fg
            selection_bg: hex(0x292E42),      // bg_highlight
            match_active_fg: hex(0x1A1B26),   // bg
            match_active_bg: hex(0xE0AF68),   // yellow
            match_inactive_bg: hex(0x24283B), // bg_dark
            edit_mode_fg: hex(0xFF9E64),      // orange
            status_bg: hex(0x16161E),         // darker status
            note: hex(0x7DCFFF),              // cyan
        }
    }

    /// Apply optional color overrides on top of this theme (in place).
    pub fn apply_overrides(&mut self, o: &ThemeColorOverrides) {
        if let Some(c) = o.add {
            self.add = c;
            self.word_add = c;
        }
        if let Some(c) = o.del {
            self.delete = c;
            self.word_del = c;
        }
        if let Some(c) = o.rail {
            self.selection_bg = c;
        }
        if let Some(c) = o.status {
            self.status_bg = c;
        }
        if let Some(c) = o.fg {
            self.selection_fg = c;
        }
        if let Some(c) = o.bg {
            // Terminal pane bg is not painted by ratatui; use as inactive
            // match / subdued chrome so the override is still visible.
            self.match_inactive_bg = c;
        }
    }
}

/// Named chrome palette preset. Independent of [`ThemeMode`] (light/dark/auto).
///
/// `Default` means "use Flexoki via [`ThemeMode`]". Named presets own their
/// light/dark character and map to the matching syntect ocean theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemePreset {
    /// Flexoki via [`ThemeMode`] (the historical default).
    #[default]
    Default,
    /// Catppuccin Mocha (dark).
    CatppuccinMocha,
    /// Catppuccin Latte (light).
    CatppuccinLatte,
    /// Tokyo Night (dark).
    TokyoNight,
}

impl ThemePreset {
    /// All built-in preset ids in docs / error messages order.
    pub const ALL: &'static [&'static str] = &[
        "default",
        "catppuccin-mocha",
        "catppuccin-latte",
        "tokyonight",
    ];

    pub fn name(self) -> &'static str {
        match self {
            ThemePreset::Default => "default",
            ThemePreset::CatppuccinMocha => "catppuccin-mocha",
            ThemePreset::CatppuccinLatte => "catppuccin-latte",
            ThemePreset::TokyoNight => "tokyonight",
        }
    }

    /// Whether this preset paints a light chrome (for syntect pairing).
    pub fn is_light(self) -> bool {
        matches!(self, ThemePreset::CatppuccinLatte)
    }

    /// Whether this preset is dark chrome. Named dark presets always are;
    /// `Default` defers to [`ThemeMode`].
    pub fn is_dark_named(self) -> bool {
        matches!(self, ThemePreset::CatppuccinMocha | ThemePreset::TokyoNight)
    }

    /// Resolve this preset + mode to a concrete [`Theme`].
    pub fn to_theme(self, mode: ThemeMode) -> Theme {
        match self {
            ThemePreset::Default => mode.to_theme(),
            ThemePreset::CatppuccinMocha => Theme::catppuccin_mocha(),
            ThemePreset::CatppuccinLatte => Theme::catppuccin_latte(),
            ThemePreset::TokyoNight => Theme::tokyonight(),
        }
    }

    /// Syntect theme for this preset + mode.
    ///
    /// Named light presets always get the light ocean theme; named dark
    /// presets always get the dark ocean theme; `Default` follows mode.
    pub fn syntect_theme_name(self, mode: ThemeMode) -> &'static str {
        match self {
            ThemePreset::Default => mode.syntect_theme_name(),
            ThemePreset::CatppuccinLatte => "base16-ocean.light",
            ThemePreset::CatppuccinMocha | ThemePreset::TokyoNight => "base16-ocean.dark",
        }
    }

    /// Parse a config/CLI string. Unknown → error with the allowed set.
    pub fn try_parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "default" | "" => Ok(ThemePreset::Default),
            "catppuccin-mocha" | "catppuccin_mocha" | "mocha" => Ok(ThemePreset::CatppuccinMocha),
            "catppuccin-latte" | "catppuccin_latte" | "latte" => Ok(ThemePreset::CatppuccinLatte),
            "tokyonight" | "tokyo-night" | "tokyo_night" => Ok(ThemePreset::TokyoNight),
            other => Err(format!(
                "unknown theme_preset '{other}' (expected {})",
                ThemePreset::ALL.join(", ")
            )),
        }
    }

    /// Parse config/CLI values. Unknown → `Default` (lenient for tests).
    pub fn parse(s: &str) -> Self {
        Self::try_parse(s).unwrap_or(ThemePreset::Default)
    }
}

/// Optional per-slot color overrides from config (`[theme_colors]` table).
///
/// Keys map onto chrome slots (not syntect):
/// - `add` / `del` — add/delete line prefixes (and word-diff accents)
/// - `rail` — selection / rail highlight background
/// - `status` — status bar background
/// - `fg` — selection foreground
/// - `bg` — subdued chrome background (match inactive)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemeColorOverrides {
    pub bg: Option<Color>,
    pub fg: Option<Color>,
    pub add: Option<Color>,
    pub del: Option<Color>,
    pub rail: Option<Color>,
    pub status: Option<Color>,
}

impl ThemeColorOverrides {
    pub fn is_empty(&self) -> bool {
        self.bg.is_none()
            && self.fg.is_none()
            && self.add.is_none()
            && self.del.is_none()
            && self.rail.is_none()
            && self.status.is_none()
    }

    /// Parse from raw string fields (config layer). Empty/`None` keys skipped.
    pub fn from_strings(
        bg: Option<&str>,
        fg: Option<&str>,
        add: Option<&str>,
        del: Option<&str>,
        rail: Option<&str>,
        status: Option<&str>,
    ) -> Result<Self, String> {
        Ok(Self {
            bg: parse_color_opt(bg, "bg")?,
            fg: parse_color_opt(fg, "fg")?,
            add: parse_color_opt(add, "add")?,
            del: parse_color_opt(del, "del")?,
            rail: parse_color_opt(rail, "rail")?,
            status: parse_color_opt(status, "status")?,
        })
    }
}

fn parse_color_opt(raw: Option<&str>, field: &str) -> Result<Option<Color>, String> {
    match raw {
        None => Ok(None),
        Some(s) if s.trim().is_empty() => Ok(None),
        Some(s) => parse_hex_color(s)
            .map(Some)
            .map_err(|e| format!("theme_colors.{field}: {e}")),
    }
}

/// Parse `#RRGGBB` / `RRGGBB` / `#RGB` / `RGB` into [`Color::Rgb`].
pub fn parse_hex_color(s: &str) -> Result<Color, String> {
    let s = s.trim();
    let hex_str = s.strip_prefix('#').unwrap_or(s);
    match hex_str.len() {
        6 => {
            let n = u32::from_str_radix(hex_str, 16)
                .map_err(|_| format!("invalid hex color '{s}' (expected #RRGGBB)"))?;
            Ok(hex(n))
        }
        3 => {
            // #RGB → #RRGGBB
            let mut expanded = String::with_capacity(6);
            for c in hex_str.chars() {
                expanded.push(c);
                expanded.push(c);
            }
            let n = u32::from_str_radix(&expanded, 16)
                .map_err(|_| format!("invalid hex color '{s}' (expected #RGB or #RRGGBB)"))?;
            Ok(hex(n))
        }
        _ => Err(format!(
            "invalid hex color '{s}' (expected #RRGGBB or #RGB)"
        )),
    }
}

/// Combined appearance used by the TUI: mode + preset + optional overrides.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThemeState {
    pub mode: ThemeMode,
    pub preset: ThemePreset,
    pub overrides: ThemeColorOverrides,
}

impl ThemeState {
    /// Resolve the concrete chrome palette (preset + mode + overrides).
    pub fn to_theme(&self) -> Theme {
        let mut theme = self.preset.to_theme(self.mode);
        theme.apply_overrides(&self.overrides);
        theme
    }

    /// Syntect theme name matching the chrome light/dark character.
    pub fn syntect_theme_name(&self) -> &'static str {
        self.preset.syntect_theme_name(self.mode)
    }

    /// Status-line / help label for the current choice.
    pub fn display_name(&self) -> String {
        match self.preset {
            ThemePreset::Default => self.mode.name().to_string(),
            other => other.name().to_string(),
        }
    }

    /// Cycle: light → auto → dark → catppuccin-mocha → catppuccin-latte →
    /// tokyonight → light. Named presets keep a sensible mode so a later
    /// switch back to `default` is not surprising.
    pub fn cycle(&self) -> Self {
        let (mode, preset) = match self.preset {
            ThemePreset::Default => match self.mode {
                ThemeMode::Light => (ThemeMode::Auto, ThemePreset::Default),
                ThemeMode::Auto => (ThemeMode::Dark, ThemePreset::Default),
                ThemeMode::Dark => (ThemeMode::Dark, ThemePreset::CatppuccinMocha),
            },
            ThemePreset::CatppuccinMocha => (ThemeMode::Light, ThemePreset::CatppuccinLatte),
            ThemePreset::CatppuccinLatte => (ThemeMode::Dark, ThemePreset::TokyoNight),
            ThemePreset::TokyoNight => (ThemeMode::Light, ThemePreset::Default),
        };
        Self {
            mode,
            preset,
            overrides: self.overrides.clone(),
        }
    }
}

/// The user's theme choice. `Auto` resolves at startup via `$COLORFGBG`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    /// Flexoki dark palette (black background).
    Dark,
    /// Flexoki light palette (paper background). The default — most reviewers
    /// read diffs on a light terminal.
    #[default]
    Light,
    /// Resolve via `$COLORFGBG`; fall back to dark when unset/unparseable.
    Auto,
}

impl ThemeMode {
    /// Cycler order for mode-only: Dark → Light → Auto → Dark.
    /// Prefer [`ThemeState::cycle`] when presets are in play.
    pub fn cycle(self) -> Self {
        match self {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Auto,
            ThemeMode::Auto => ThemeMode::Dark,
        }
    }

    /// Lowercase name for the status line.
    pub fn name(self) -> &'static str {
        match self {
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
            ThemeMode::Auto => "auto",
        }
    }

    /// Resolve this mode to a concrete Flexoki [`Theme`]. `Auto` inspects
    /// `$COLORFGBG` once.
    pub fn to_theme(self) -> Theme {
        match self {
            ThemeMode::Dark => Theme::dark(),
            ThemeMode::Light => Theme::light(),
            ThemeMode::Auto => resolve_auto(),
        }
    }

    /// Return the syntect theme name that matches this mode:
    /// `"base16-ocean.dark"` for dark backgrounds,
    /// `"base16-ocean.light"` for light backgrounds.
    pub fn syntect_theme_name(self) -> &'static str {
        match self {
            ThemeMode::Dark => "base16-ocean.dark",
            ThemeMode::Light => "base16-ocean.light",
            ThemeMode::Auto => {
                if background_is_light() {
                    "base16-ocean.light"
                } else {
                    "base16-ocean.dark"
                }
            }
        }
    }

    /// Parse a config string (`"dark"` / `"light"` / `"auto"`) into a mode.
    /// Unknown / empty values fall back to [`ThemeMode::Light`] (the default),
    /// so a typo never breaks the TUI.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "dark" => ThemeMode::Dark,
            "light" => ThemeMode::Light,
            "auto" => ThemeMode::Auto,
            _ => ThemeMode::Light,
        }
    }

    /// Strict parse used by config validation.
    pub fn try_parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dark" => Ok(ThemeMode::Dark),
            "light" => Ok(ThemeMode::Light),
            "auto" => Ok(ThemeMode::Auto),
            other => Err(format!(
                "unknown theme '{other}' (expected dark, light, or auto)"
            )),
        }
    }
}

/// Resolve the `auto` theme from the `$COLORFGBG` environment variable.
///
/// `$COLORFGBG` is a convention (set by some terminals, e.g. xterm, rxvt,
/// iTerm2) formatted as `"fg;bg"` where each field is a 0–15 ANSI color index.
/// The standard interpretation: bg index ≥ 7 means a light background.
///
/// Returns [`Theme::dark()`] when the variable is unset or unparseable — this
/// is best-effort detection with zero I/O, so it never blocks.
pub fn resolve_auto() -> Theme {
    if background_is_light() {
        Theme::light()
    } else {
        Theme::dark()
    }
}

/// Inspect `$COLORFGBG` and decide whether the terminal background looks
/// light. Exposed for testing (the env-var read is isolated here so tests can
/// set/unset the var around it).
pub fn background_is_light() -> bool {
    let Some(val) = std::env::var_os("COLORFGBG") else {
        return false;
    };
    let Some(val) = val.to_str() else {
        return false;
    };
    // Format: "fg;bg" (some terminals append ";default", so split on ';'
    // and take the first two fields).
    let mut parts = val.split(';');
    let _fg = parts.next();
    let bg = parts.next();
    match bg.and_then(|b| b.parse::<u32>().ok()) {
        Some(n) => n >= 7,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `body` with `$COLORFGBG` set to `val` (or unset if `None`),
    /// restoring the previous value afterward.
    fn with_colorfgbg<T>(val: Option<&str>, body: impl FnOnce() -> T) -> T {
        let prev = std::env::var_os("COLORFGBG");
        match val {
            Some(v) => std::env::set_var("COLORFGBG", v),
            None => std::env::remove_var("COLORFGBG"),
        }
        let result = body();
        match prev {
            Some(p) => std::env::set_var("COLORFGBG", p),
            None => std::env::remove_var("COLORFGBG"),
        }
        result
    }

    #[test]
    fn dark_and_light_palettes_differ() {
        let d = Theme::dark();
        let l = Theme::light();
        assert_ne!(d.file_header, l.file_header);
        assert_ne!(d.hunk_header, l.hunk_header);
        assert_ne!(d.status_bg, l.status_bg);
        assert_ne!(d.word_add, l.word_add);
        assert_ne!(d.match_inactive_bg, l.match_inactive_bg);
    }

    #[test]
    fn mode_cycle_order() {
        assert_eq!(ThemeMode::Dark.cycle(), ThemeMode::Light);
        assert_eq!(ThemeMode::Light.cycle(), ThemeMode::Auto);
        assert_eq!(ThemeMode::Auto.cycle(), ThemeMode::Dark);
    }

    #[test]
    fn mode_names() {
        assert_eq!(ThemeMode::Dark.name(), "dark");
        assert_eq!(ThemeMode::Light.name(), "light");
        assert_eq!(ThemeMode::Auto.name(), "auto");
    }

    #[test]
    fn parse_known_and_unknown() {
        assert_eq!(ThemeMode::parse("dark"), ThemeMode::Dark);
        assert_eq!(ThemeMode::parse("Light"), ThemeMode::Light);
        assert_eq!(ThemeMode::parse("AUTO"), ThemeMode::Auto);
        assert_eq!(ThemeMode::parse("nonsense"), ThemeMode::Light);
        assert_eq!(ThemeMode::parse(""), ThemeMode::Light);
        assert_eq!(ThemeMode::parse("  auto "), ThemeMode::Auto);
    }

    #[test]
    fn to_theme_resolves_each_mode() {
        assert_eq!(ThemeMode::Dark.to_theme().add, Theme::dark().add);
        assert_eq!(ThemeMode::Light.to_theme().add, Theme::light().add);
    }

    #[test]
    fn background_is_light_when_bg_index_high() {
        with_colorfgbg(Some("0;15"), || {
            assert!(background_is_light());
        });
        with_colorfgbg(Some("15;7"), || {
            assert!(background_is_light());
        });
    }

    #[test]
    fn background_is_dark_when_bg_index_low() {
        with_colorfgbg(Some("7;0"), || {
            assert!(!background_is_light());
        });
        with_colorfgbg(Some("0;6"), || {
            assert!(!background_is_light());
        });
    }

    #[test]
    fn background_is_dark_when_unset() {
        with_colorfgbg(None, || {
            assert!(!background_is_light());
        });
    }

    #[test]
    fn background_is_dark_when_malformed() {
        with_colorfgbg(Some("not a number"), || {
            assert!(!background_is_light());
        });
        with_colorfgbg(Some("0;abc"), || {
            assert!(!background_is_light());
        });
        with_colorfgbg(Some("0;7;default"), || {
            assert!(background_is_light());
        });
    }

    #[test]
    fn resolve_auto_follows_env() {
        with_colorfgbg(Some("0;15"), || {
            assert_eq!(resolve_auto().add, Theme::light().add);
        });
        with_colorfgbg(Some("0;0"), || {
            assert_eq!(resolve_auto().add, Theme::dark().add);
        });
        with_colorfgbg(None, || {
            assert_eq!(resolve_auto().add, Theme::dark().add);
        });
    }

    #[test]
    fn syntect_theme_name_dark() {
        assert_eq!(ThemeMode::Dark.syntect_theme_name(), "base16-ocean.dark");
    }

    #[test]
    fn syntect_theme_name_light() {
        assert_eq!(ThemeMode::Light.syntect_theme_name(), "base16-ocean.light");
    }

    #[test]
    fn syntect_theme_name_auto_follows_bg() {
        with_colorfgbg(Some("0;15"), || {
            assert_eq!(ThemeMode::Auto.syntect_theme_name(), "base16-ocean.light");
        });
        with_colorfgbg(Some("0;0"), || {
            assert_eq!(ThemeMode::Auto.syntect_theme_name(), "base16-ocean.dark");
        });
    }

    #[test]
    fn presets_differ_from_flexoki() {
        let mocha = Theme::catppuccin_mocha();
        let latte = Theme::catppuccin_latte();
        let tokyo = Theme::tokyonight();
        assert_ne!(mocha.add, Theme::dark().add);
        assert_ne!(latte.add, Theme::light().add);
        assert_ne!(tokyo.add, Theme::dark().add);
        assert_ne!(mocha.status_bg, latte.status_bg);
    }

    #[test]
    fn light_preset_uses_light_syntect() {
        assert_eq!(
            ThemePreset::CatppuccinLatte.syntect_theme_name(ThemeMode::Dark),
            "base16-ocean.light"
        );
        // Dark mode must not force dark syntect under a light preset.
        assert!(!ThemePreset::CatppuccinLatte
            .syntect_theme_name(ThemeMode::Dark)
            .contains("dark"));
    }

    #[test]
    fn dark_presets_use_dark_syntect() {
        assert_eq!(
            ThemePreset::CatppuccinMocha.syntect_theme_name(ThemeMode::Light),
            "base16-ocean.dark"
        );
        assert_eq!(
            ThemePreset::TokyoNight.syntect_theme_name(ThemeMode::Light),
            "base16-ocean.dark"
        );
    }

    #[test]
    fn theme_state_cycle_includes_presets() {
        let mut s = ThemeState {
            mode: ThemeMode::Light,
            preset: ThemePreset::Default,
            overrides: ThemeColorOverrides::default(),
        };
        s = s.cycle();
        assert_eq!(s.mode, ThemeMode::Auto);
        assert_eq!(s.preset, ThemePreset::Default);
        s = s.cycle();
        assert_eq!(s.mode, ThemeMode::Dark);
        s = s.cycle();
        assert_eq!(s.preset, ThemePreset::CatppuccinMocha);
        s = s.cycle();
        assert_eq!(s.preset, ThemePreset::CatppuccinLatte);
        s = s.cycle();
        assert_eq!(s.preset, ThemePreset::TokyoNight);
        s = s.cycle();
        assert_eq!(s.preset, ThemePreset::Default);
        assert_eq!(s.mode, ThemeMode::Light);
    }

    #[test]
    fn overrides_apply_to_slots() {
        let mut theme = Theme::dark();
        let o = ThemeColorOverrides {
            add: Some(hex(0x00FF00)),
            del: Some(hex(0xFF0000)),
            rail: Some(hex(0x111111)),
            status: Some(hex(0x222222)),
            fg: Some(hex(0xEEEEEE)),
            bg: Some(hex(0x010101)),
        };
        theme.apply_overrides(&o);
        assert_eq!(theme.add, hex(0x00FF00));
        assert_eq!(theme.word_add, hex(0x00FF00));
        assert_eq!(theme.delete, hex(0xFF0000));
        assert_eq!(theme.selection_bg, hex(0x111111));
        assert_eq!(theme.status_bg, hex(0x222222));
        assert_eq!(theme.selection_fg, hex(0xEEEEEE));
        assert_eq!(theme.match_inactive_bg, hex(0x010101));
    }

    #[test]
    fn parse_hex_color_accepts_hash_and_short() {
        assert_eq!(parse_hex_color("#A6E3A1").unwrap(), hex(0xA6E3A1));
        assert_eq!(parse_hex_color("A6E3A1").unwrap(), hex(0xA6E3A1));
        assert_eq!(parse_hex_color("#abc").unwrap(), hex(0xAABBCC));
        assert!(parse_hex_color("zz").is_err());
        assert!(parse_hex_color("").is_err());
    }

    #[test]
    fn theme_preset_try_parse() {
        assert_eq!(
            ThemePreset::try_parse("catppuccin-mocha").unwrap(),
            ThemePreset::CatppuccinMocha
        );
        assert_eq!(
            ThemePreset::try_parse("mocha").unwrap(),
            ThemePreset::CatppuccinMocha
        );
        assert_eq!(
            ThemePreset::try_parse("latte").unwrap(),
            ThemePreset::CatppuccinLatte
        );
        assert_eq!(
            ThemePreset::try_parse("tokyonight").unwrap(),
            ThemePreset::TokyoNight
        );
        assert_eq!(
            ThemePreset::try_parse("default").unwrap(),
            ThemePreset::Default
        );
        assert!(ThemePreset::try_parse("solarized").is_err());
    }

    #[test]
    fn display_name_prefers_preset() {
        let s = ThemeState {
            mode: ThemeMode::Light,
            preset: ThemePreset::TokyoNight,
            overrides: ThemeColorOverrides::default(),
        };
        assert_eq!(s.display_name(), "tokyonight");
        let s2 = ThemeState::default();
        assert_eq!(s2.display_name(), "light");
    }
}
