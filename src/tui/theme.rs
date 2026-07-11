//! TUI color theme — semantic color slots, not raw [`Color`] literals.
//!
//! `view.rs` reads colors from [`Theme`] instead of hardcoding them, so the
//! chrome adapts to a light vs. dark terminal background. [`ThemeMode`] holds
//! the user's choice (dark / light / auto); `auto` resolves via the
//! `$COLORFGBG` convention at startup.
//!
//! This themes only the TUI chrome (line prefixes, gutters, status bar, …).
//! Syntax-highlight (syntect) keeps its own default theme; swapping that is a
//! follow-up.

use ratatui::style::Color;

/// A set of semantic color slots covering everything `view.rs` paints.
///
/// Colors are stored by value (small, `Copy`) so the view can read them
/// cheaply per cell.
#[derive(Debug, Clone, Copy)]
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
}

impl Theme {
    /// Palette tuned for a dark terminal background.
    pub fn dark() -> Self {
        Self {
            add: Color::Green,
            delete: Color::Red,
            word_add: Color::LightGreen,
            word_del: Color::LightRed,
            dim: Color::DarkGray,
            file_header: Color::Yellow,
            hunk_header: Color::Blue,
            selection_fg: Color::Black,
            selection_bg: Color::Cyan,
            match_active_fg: Color::Black,
            match_active_bg: Color::Yellow,
            match_inactive_bg: Color::DarkGray,
            edit_mode_fg: Color::Yellow,
            status_bg: Color::Black,
        }
    }

    /// Palette tuned for a light terminal background (white/light-gray bg).
    /// Uses the brighter `Light*` variants for readable contrast on white.
    pub fn light() -> Self {
        Self {
            add: Color::LightGreen,
            delete: Color::LightRed,
            word_add: Color::Green,
            word_del: Color::Red,
            dim: Color::Gray,
            file_header: Color::LightYellow,
            hunk_header: Color::LightBlue,
            selection_fg: Color::Black,
            selection_bg: Color::LightCyan,
            match_active_fg: Color::Black,
            match_active_bg: Color::LightYellow,
            match_inactive_bg: Color::Gray,
            edit_mode_fg: Color::LightYellow,
            status_bg: Color::White,
        }
    }
}

/// The user's theme choice. `Auto` resolves at startup via `$COLORFGBG`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    /// Dark palette (the default — matches prior behavior).
    #[default]
    Dark,
    /// Light palette for white/light terminals.
    Light,
    /// Resolve via `$COLORFGBG`; fall back to dark when unset/unparseable.
    Auto,
}

impl ThemeMode {
    /// Cycler order: Dark → Light → Auto → Dark.
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

    /// Resolve this mode to a concrete [`Theme`]. `Auto` inspects
    /// `$COLORFGBG` once.
    pub fn to_theme(self) -> Theme {
        match self {
            ThemeMode::Dark => Theme::dark(),
            ThemeMode::Light => Theme::light(),
            ThemeMode::Auto => resolve_auto(),
        }
    }

    /// Parse a config string (`"dark"` / `"light"` / `"auto"`) into a mode.
    /// Unknown / empty values fall back to [`ThemeMode::Dark`] (the default),
    /// so a typo never breaks the TUI.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "light" => ThemeMode::Light,
            "auto" => ThemeMode::Auto,
            _ => ThemeMode::Dark,
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
        assert_ne!(d.add, l.add);
        assert_ne!(d.dim, l.dim);
        assert_ne!(d.status_bg, l.status_bg);
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
        assert_eq!(ThemeMode::parse("nonsense"), ThemeMode::Dark);
        assert_eq!(ThemeMode::parse(""), ThemeMode::Dark);
        assert_eq!(ThemeMode::parse("  auto "), ThemeMode::Auto);
    }

    #[test]
    fn to_theme_resolves_each_mode() {
        // Dark/Light are deterministic; Auto depends on env (tested below).
        assert_eq!(ThemeMode::Dark.to_theme().add, Theme::dark().add);
        assert_eq!(ThemeMode::Light.to_theme().add, Theme::light().add);
    }

    #[test]
    fn background_is_light_when_bg_index_high() {
        with_colorfgbg(Some("0;15"), || {
            assert!(background_is_light());
        });
        with_colorfgbg(Some("15;7"), || {
            assert!(background_is_light()); // exactly 7 counts as light
        });
    }

    #[test]
    fn background_is_dark_when_bg_index_low() {
        with_colorfgbg(Some("7;0"), || {
            assert!(!background_is_light());
        });
        with_colorfgbg(Some("0;6"), || {
            assert!(!background_is_light()); // below 7
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
        // Trailing ";default" (iTerm2) is tolerated.
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
}
