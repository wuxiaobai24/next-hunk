//! TUI color theme — semantic color slots, not raw [`Color`] literals.
//!
//! `view.rs` reads colors from [`Theme`] instead of hardcoding them, so the
//! chrome adapts to a light vs. dark terminal background. [`ThemeMode`] holds
//! the user's choice (dark / light / auto); `auto` resolves via the
//! `$COLORFGBG` convention at startup.
//!
//! Both palettes are [Flexoki](https://flexoki.com) — an inky, contrast-balanced
//! color system by Steph Ango — mapped onto the semantic slots. The light
//! variant uses Flexoki's paper background (`#FFFCF0`) with the deeper 600-level
//! accents; the dark variant uses the black background (`#100F0F`) with the
//! brighter 400-level accents.
//!
//! This themes only the TUI chrome (line prefixes, gutters, status bar, …).
//! Syntax-highlight (syntect) keeps its own default theme; swapping that is a
//! follow-up.

use ratatui::style::Color;

/// Build a `Color::Rgb` from a `0xRRGGBB` literal, so the Flexoki palette reads
/// as hex rather than three loose bytes. `const` so it costs nothing at runtime.
const fn hex(c: u32) -> Color {
    Color::Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

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
    /// Agent annotation text (`--note` rows).
    pub note: Color,
}

impl Theme {
    /// Flexoki dark variant — tuned for a black terminal background
    /// (`#100F0F`), using the brighter 400-level accents.
    pub fn dark() -> Self {
        Self {
            add: hex(0x879A39), // green-400
            delete: hex(0xD14D41), // red-400
            // Word-level emphasis is rendered reversed+bold, so the brighter
            // 300-level shades become solid color blocks that pop on dark.
            word_add: hex(0xA0AF54), // green-300
            word_del: hex(0xE8705F), // red-300
            dim: hex(0x878580), // base-500 (gutter, meta, help line)
            file_header: hex(0xCE5D97), // magenta-400 (bold)
            hunk_header: hex(0x4385BE), // blue-400 (bold)
            selection_fg: hex(0xFFFCF0), // paper
            selection_bg: hex(0x575653), // base-700 (rail bar)
            match_active_fg: hex(0x100F0F), // black
            match_active_bg: hex(0xDFB431), // yellow-300 (gold match)
            match_inactive_bg: hex(0x403E3C), // base-800 (subdued)
            edit_mode_fg: hex(0xDA702C), // orange-400 (active prompt)
            status_bg: hex(0x282726), // base-900 (status band)
            note: hex(0x3AA99F), // cyan-400 (italic agent notes)
        }
    }

    /// Flexoki light variant — tuned for a paper terminal background
    /// (`#FFFCF0`), using the deeper 600-level accents which stay readable on
    /// white/paper. (The pale `Light*` ANSI variants are meant for dark
    /// backgrounds and go invisible on paper, so we avoid them.) Word-level
    /// emphasis reuses brighter shades; it renders reversed+bold, so it still
    /// pops without fighting the line color.
    pub fn light() -> Self {
        Self {
            add: hex(0x66800B), // green-600
            delete: hex(0xAF3029), // red-600
            word_add: hex(0x879A39), // green-400
            word_del: hex(0xD14D41), // red-400
            dim: hex(0x6F6E69), // base-600 (gutter, meta, help line)
            file_header: hex(0xA02F6F), // magenta-600 (bold)
            hunk_header: hex(0x205EA6), // blue-600 (bold)
            selection_fg: hex(0x100F0F), // black ink
            selection_bg: hex(0xCECDC3), // base-200 (rail bar)
            match_active_fg: hex(0x100F0F), // black
            match_active_bg: hex(0xDFB431), // yellow-300 (gold match)
            match_inactive_bg: hex(0xCECDC3), // base-200 (subdued)
            edit_mode_fg: hex(0xBC5215), // orange-600 (active prompt)
            status_bg: hex(0xE6E4D9), // base-100 (status band)
            note: hex(0x24837B), // cyan-600 (italic agent notes)
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
        // Sanity: the two Flexoki variants are genuinely distinct. We assert on
        // slots that intentionally differ (dark uses 400-level accents on black;
        // light uses 600-level accents on paper).
        assert_ne!(d.file_header, l.file_header); // magenta-400 vs magenta-600
        assert_ne!(d.hunk_header, l.hunk_header); // blue-400 vs blue-600
        assert_ne!(d.status_bg, l.status_bg); // base-900 vs base-100
        assert_ne!(d.word_add, l.word_add); // green-300 vs green-400
        assert_ne!(d.match_inactive_bg, l.match_inactive_bg); // base-800 vs base-200
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
        // Unknown / empty falls back to the default (Light).
        assert_eq!(ThemeMode::parse("nonsense"), ThemeMode::Light);
        assert_eq!(ThemeMode::parse(""), ThemeMode::Light);
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
