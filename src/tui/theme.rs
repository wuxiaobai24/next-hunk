//! TUI color theme — semantic color slots, not raw [`Color`] literals.
//!
//! `view.rs` reads colors from [`Theme`] instead of hardcoding them, so the
//! chrome adapts to a light vs. dark terminal background. [`ThemeMode`] holds
//! the user's choice (dark / light / auto); `auto` resolves via the
//! `$COLORFGBG` convention at startup.
//!
//! The default palette is [Flexoki](https://flexoki.com) — an inky,
//! contrast-balanced color system by Steph Ango — mapped onto the semantic
//! slots (light: paper background + 600-level accents; dark: black
//! background + 400-level accents).
//!
//! Curated presets ([`Palette`]) add Catppuccin, Gruvbox, Nord, and Tokyo
//! Night for the terminals people actually run.
//!
//! This themes the TUI chrome (line prefixes, gutters, status bar, …) and
//! picks the closest matching syntect syntax theme per palette.

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
    /// Background of the cursor row (the `j/k`-moving review cursor).
    pub cursor_bg: Color,
    /// Status-bar text color when editing a query/filter.
    pub edit_mode_fg: Color,
    /// Status-bar background.
    pub status_bg: Color,
    /// Agent annotation text (`--note` rows).
    pub note: Color,
    /// Foreground that stays readable painted *on top of* a solid accent
    /// fill (the `add` / `delete` / `hunk_header` slots). Dark palettes pick
    /// their ink (their accents are mid-tone), light palettes pick a
    /// near-white (their accents are deep). Not for the gold match fill —
    /// that pairs with `match_active_fg`, which is dark on every palette.
    pub on_accent: Color,
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
            cursor_bg: hex(0x575653),         // base-700 (review cursor row)
            edit_mode_fg: hex(0xDA702C),      // orange-400 (active prompt)
            status_bg: hex(0x282726),         // base-900 (status band)
            note: hex(0x3AA99F),              // cyan-400 (italic agent notes)
            on_accent: hex(0x100F0F),         // ink over mid-tone 400-level accents
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
            cursor_bg: hex(0xB7B5AC),         // base-300 (review cursor row)
            edit_mode_fg: hex(0xBC5215),      // orange-600 (active prompt)
            status_bg: hex(0xE6E4D9),         // base-100 (status band)
            note: hex(0x24837B),              // cyan-600 (italic agent notes)
            on_accent: hex(0xFFFCF0),         // paper over deep 600-level accents
        }
    }

    /// Catppuccin Mocha — dark, pastel-on-cold. Mapped from the official
    /// palette (base `#1E1E2E`, mantle `#181825`, surface `#313244`/`#45475A`).
    pub fn catppuccin_mocha() -> Self {
        Self {
            add: hex(0xA6E3A1),               // green
            delete: hex(0xF38BA8),            // red
            word_add: hex(0xA6E3A1),          // green (rendered reversed+bold)
            word_del: hex(0xF38BA8),          // red
            dim: hex(0x6C7086),               // overlay0
            file_header: hex(0xCBA6F7),       // mauve (bold)
            hunk_header: hex(0x89B4FA),       // blue (bold)
            selection_fg: hex(0xCDD6F4),      // text
            selection_bg: hex(0x45475A),      // surface1 (rail bar)
            match_active_fg: hex(0x11111B),   // crust
            match_active_bg: hex(0xF9E2AF),   // yellow (gold match)
            match_inactive_bg: hex(0x313244), // surface0 (subdued)
            cursor_bg: hex(0x45475A),         // surface1 (review cursor row)
            edit_mode_fg: hex(0xFAB387),      // peach (active prompt)
            status_bg: hex(0x181825),         // mantle (status band)
            note: hex(0x94E2D5),              // teal (italic agent notes)
            on_accent: hex(0x11111B),         // crust over light pastel accents
        }
    }

    /// Catppuccin Latte — the light sibling of Mocha (base `#EFF1F5`,
    /// mantle `#E6E9EF`, surface `#CCD0DA`/`#BCC0CC`).
    pub fn catppuccin_latte() -> Self {
        Self {
            add: hex(0x40A02B),               // green
            delete: hex(0xD20F39),            // red
            word_add: hex(0x40A02B),          // green
            word_del: hex(0xD20F39),          // red
            dim: hex(0x6C6F85),               // subtext0
            file_header: hex(0x8839EF),       // mauve (bold)
            hunk_header: hex(0x1E66F5),       // blue (bold)
            selection_fg: hex(0x4C4F69),      // text
            selection_bg: hex(0xBCC0CC),      // surface1 (rail bar)
            match_active_fg: hex(0x4C4F69),   // text on gold
            match_active_bg: hex(0xDF8E1D),   // yellow (gold match)
            match_inactive_bg: hex(0xCCD0DA), // surface0 (subdued)
            cursor_bg: hex(0xBCC0CC),         // surface1 (review cursor row)
            edit_mode_fg: hex(0xFE640B),      // peach (active prompt)
            status_bg: hex(0xE6E9EF),         // mantle (status band)
            note: hex(0x179299),              // teal (italic agent notes)
            on_accent: hex(0xFFFFFF),         // white over deep accent fills
        }
    }

    /// Gruvbox Dark — warm, earthy retro (bg `#282828`, fg `#EBDBB2`).
    pub fn gruvbox_dark() -> Self {
        Self {
            add: hex(0xB8BB26),               // green
            delete: hex(0xFB4934),            // red
            word_add: hex(0xB8BB26),          // green
            word_del: hex(0xFB4934),          // red
            dim: hex(0xA89984),               // fg4 (gutter, meta, help line)
            file_header: hex(0xD3869B),       // purple (bold)
            hunk_header: hex(0x83A598),       // blue (bold)
            selection_fg: hex(0xEBDBB2),      // fg
            selection_bg: hex(0x504945),      // bg2 (rail bar)
            match_active_fg: hex(0x282828),   // bg0
            match_active_bg: hex(0xFABD2F),   // yellow (gold match)
            match_inactive_bg: hex(0x3C3836), // bg1 (subdued)
            cursor_bg: hex(0x504945),         // bg2 (review cursor row)
            edit_mode_fg: hex(0xFE8019),      // orange (active prompt)
            status_bg: hex(0x3C3836),         // bg1 (status band)
            note: hex(0x8EC07C),              // aqua (italic agent notes)
            on_accent: hex(0x282828),         // bg0 over mid-tone accents
        }
    }

    /// Nord — cool, arctic-blue (polar night bg `#2E3440`, snow fg
    /// `#ECEFF4`, frost blues `#81A1C1`/`#88C0D0`, aurora accents).
    pub fn nord() -> Self {
        Self {
            add: hex(0xA3BE8C),               // nord14 (green)
            delete: hex(0xBF616A),            // nord11 (red)
            word_add: hex(0xA3BE8C),          // nord14
            word_del: hex(0xBF616A),          // nord11
            dim: hex(0x4C566A),               // nord3 (gutter, meta, help line)
            file_header: hex(0xB48EAD),       // nord15 (purple, bold)
            hunk_header: hex(0x81A1C1),       // nord9 (frost blue, bold)
            selection_fg: hex(0xECEFF4),      // nord6 (snow)
            selection_bg: hex(0x434C5E),      // nord2 (rail bar)
            match_active_fg: hex(0x2E3440),   // nord0
            match_active_bg: hex(0xEBCB8B),   // nord13 (gold match)
            match_inactive_bg: hex(0x3B4252), // nord1 (subdued)
            cursor_bg: hex(0x434C5E),         // nord2 (review cursor row)
            edit_mode_fg: hex(0xD08770),      // nord12 (orange, active prompt)
            status_bg: hex(0x3B4252),         // nord1 (status band)
            note: hex(0x88C0D0),              // nord8 (frost cyan, italic notes)
            on_accent: hex(0x2E3440),         // nord0 over aurora accents
        }
    }

    /// Tokyo Night — deep blue night city (bg `#1A1B26`, fg `#C0CAF5`,
    /// highlight `#292E42`, comment `#565F89`).
    pub fn tokyonight() -> Self {
        Self {
            add: hex(0x9ECE6A),               // green
            delete: hex(0xF7768E),            // red
            word_add: hex(0x9ECE6A),          // green
            word_del: hex(0xF7768E),          // red
            dim: hex(0x565F89),               // comment (gutter, meta, help line)
            file_header: hex(0xBB9AF7),       // magenta (bold)
            hunk_header: hex(0x7AA2F7),       // blue (bold)
            selection_fg: hex(0xC0CAF5),      // fg
            selection_bg: hex(0x292E42),      // bg_highlight (rail bar)
            match_active_fg: hex(0x1A1B26),   // bg
            match_active_bg: hex(0xE0AF68),   // yellow (gold match)
            match_inactive_bg: hex(0x414868), // terminal_black (subdued)
            cursor_bg: hex(0x292E42),         // bg_highlight (review cursor row)
            edit_mode_fg: hex(0xFF9E64),      // orange (active prompt)
            status_bg: hex(0x16161E),         // bg_dark (status band)
            note: hex(0x7DCFFF),              // cyan (italic agent notes)
            on_accent: hex(0x1A1B26),         // bg over mid-tone accents
        }
    }
}

/// A curated chrome palette family. Each family provides a dark variant
/// (Flexoki, Catppuccin, Gruvbox, Nord, Tokyo Night also offer light where
/// the source palette defines one) and the closest syntect syntax theme.
///
/// Config: `theme = "catppuccin-mocha"`, `"gruvbox-dark"`, `"nord"`,
/// `"tokyonight"`, `"flexoki"` / `"flexoki-light"` — plus the legacy
/// `"dark"` / `"light"` / `"auto"` (Flexoki + mode). Runtime: `T` cycles
/// families, `t` cycles dark/light/auto within the family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Palette {
    #[default]
    Flexoki,
    Catppuccin,
    Gruvbox,
    Nord,
    TokyoNight,
}

impl Palette {
    /// Cycler order for `T`: Flexoki → Catppuccin → Gruvbox → Nord →
    /// Tokyo Night → Flexoki.
    pub fn cycle(self) -> Self {
        match self {
            Palette::Flexoki => Palette::Catppuccin,
            Palette::Catppuccin => Palette::Gruvbox,
            Palette::Gruvbox => Palette::Nord,
            Palette::Nord => Palette::TokyoNight,
            Palette::TokyoNight => Palette::Flexoki,
        }
    }

    /// Short family name (status line, parse input).
    pub fn name(self) -> &'static str {
        match self {
            Palette::Flexoki => "flexoki",
            Palette::Catppuccin => "catppuccin",
            Palette::Gruvbox => "gruvbox",
            Palette::Nord => "nord",
            Palette::TokyoNight => "tokyonight",
        }
    }

    /// The full preset name shown in the status line, e.g.
    /// `catppuccin-mocha`. Mode maps to the family's variant; families
    /// without a light variant keep their dark one in light mode.
    pub fn preset_name(self, mode: ThemeMode) -> String {
        let mode = if mode == ThemeMode::Auto {
            if background_is_light() {
                ThemeMode::Light
            } else {
                ThemeMode::Dark
            }
        } else {
            mode
        };
        match (self, mode) {
            (Palette::Flexoki, ThemeMode::Dark) => "flexoki".into(),
            (Palette::Flexoki, _) => "flexoki-light".into(),
            (Palette::Catppuccin, ThemeMode::Dark) => "catppuccin-mocha".into(),
            (Palette::Catppuccin, _) => "catppuccin-latte".into(),
            (Palette::Gruvbox, _) => "gruvbox-dark".into(),
            (Palette::Nord, _) => "nord".into(),
            (Palette::TokyoNight, _) => "tokyonight".into(),
        }
    }

    /// Resolve the family + mode to a concrete [`Theme`].
    pub fn theme(self, mode: ThemeMode) -> Theme {
        let light = match mode {
            ThemeMode::Dark => false,
            ThemeMode::Light => true,
            ThemeMode::Auto => background_is_light(),
        };
        match self {
            Palette::Flexoki => {
                if light {
                    Theme::light()
                } else {
                    Theme::dark()
                }
            }
            Palette::Catppuccin => {
                if light {
                    Theme::catppuccin_latte()
                } else {
                    Theme::catppuccin_mocha()
                }
            }
            // Single-variant families: no official light palette shipped,
            // so light mode keeps the dark one.
            Palette::Gruvbox => Theme::gruvbox_dark(),
            Palette::Nord => Theme::nord(),
            Palette::TokyoNight => Theme::tokyonight(),
        }
    }

    /// The closest syntect syntax theme for this family + mode, chosen from
    /// syntect's built-in set.
    pub fn syntect_theme_name(self, mode: ThemeMode) -> &'static str {
        let light = match mode {
            ThemeMode::Dark => false,
            ThemeMode::Light => true,
            ThemeMode::Auto => background_is_light(),
        };
        match self {
            Palette::Flexoki => {
                if light {
                    "base16-ocean.light"
                } else {
                    "base16-ocean.dark"
                }
            }
            Palette::Catppuccin => {
                if light {
                    "InspiredGitHub"
                } else {
                    "base16-mocha.dark"
                }
            }
            Palette::Gruvbox => "base16-eighties.dark",
            Palette::Nord => "base16-ocean.dark",
            Palette::TokyoNight => "base16-ocean.dark",
        }
    }
}

/// Parse a `theme = "…"` config value into (palette, mode). Accepts preset
/// names (`catppuccin-mocha`, `gruvbox-dark`, `nord`, `tokyonight`,
/// `flexoki`/`flexoki-light`), the light variants of those families
/// (`catppuccin-latte`), and the legacy mode names (`dark`/`light`/`auto`,
/// Flexoki). Unknown values fall back to the default (Flexoki dark), so a
/// typo never breaks the TUI.
pub fn parse_theme(s: &str) -> (Palette, ThemeMode) {
    match s.trim().to_ascii_lowercase().as_str() {
        "dark" => (Palette::Flexoki, ThemeMode::Dark),
        "light" => (Palette::Flexoki, ThemeMode::Light),
        "auto" => (Palette::Flexoki, ThemeMode::Auto),
        "flexoki" | "flexoki-dark" => (Palette::Flexoki, ThemeMode::Dark),
        "flexoki-light" => (Palette::Flexoki, ThemeMode::Light),
        "catppuccin" | "catppuccin-mocha" => (Palette::Catppuccin, ThemeMode::Dark),
        "catppuccin-latte" => (Palette::Catppuccin, ThemeMode::Light),
        "gruvbox" | "gruvbox-dark" => (Palette::Gruvbox, ThemeMode::Dark),
        "nord" => (Palette::Nord, ThemeMode::Dark),
        "tokyonight" | "tokyonight-night" => (Palette::TokyoNight, ThemeMode::Dark),
        _ => (Palette::Flexoki, ThemeMode::Dark),
    }
}

/// True when `s` is a token [`parse_theme`] understands. Used at startup to
/// warn on a typo (`theme = "cattpuccin"`) instead of silently falling back
/// to the default theme.
pub fn theme_known(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "dark"
            | "light"
            | "auto"
            | "flexoki"
            | "flexoki-dark"
            | "flexoki-light"
            | "catppuccin"
            | "catppuccin-mocha"
            | "catppuccin-latte"
            | "gruvbox"
            | "gruvbox-dark"
            | "nord"
            | "tokyonight"
            | "tokyonight-night"
    )
}

/// The user's theme choice. `Auto` resolves at startup via `$COLORFGBG`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    /// Flexoki dark palette (black background) — the default, and the look
    /// the bare `"flexoki"` preset name stands for.
    #[default]
    Dark,
    /// Flexoki light palette (paper background).
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
    /// Unknown / empty values fall back to [`ThemeMode::Dark`] (the default),
    /// so a typo never breaks the TUI.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "dark" => ThemeMode::Dark,
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

/// WCAG-2 relative-luminance contrast helpers shared by the theme and view
/// test modules — the regression gate for "text stays readable on every
/// palette's solid fills, light and dark".
#[cfg(test)]
pub(crate) mod test_support {
    use ratatui::style::Color;

    fn channel(v: u8) -> f64 {
        let v = f64::from(v) / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }

    /// WCAG-2 relative luminance. Panics on non-`Rgb` colors: every themed
    /// slot is `hex(..)`-built, so hitting another variant means a palette
    /// regressed to an ANSI literal.
    pub fn relative_luminance(c: Color) -> f64 {
        let Color::Rgb(r, g, b) = c else {
            panic!("non-Rgb color {c:?} in a theme slot");
        };
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    /// WCAG-2 contrast ratio (1.0 = identical, 21.0 = black on white).
    pub fn contrast(a: Color, b: Color) -> f64 {
        let (la, lb) = (relative_luminance(a), relative_luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
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
    fn theme_known_covers_every_parse_theme_token() {
        for s in [
            "dark",
            "light",
            "auto",
            "flexoki",
            "flexoki-dark",
            "flexoki-light",
            "catppuccin",
            "catppuccin-mocha",
            "catppuccin-latte",
            "gruvbox",
            "gruvbox-dark",
            "nord",
            "tokyonight",
            "tokyonight-night",
            " Flexoki ",
        ] {
            assert!(theme_known(s), "{s} should be known");
        }
        for s in ["", "banana", "cattpuccin", "flexoki-darkk"] {
            assert!(!theme_known(s), "{s} should be unknown");
        }
    }

    #[test]
    fn parse_theme_accepts_presets_and_legacy_modes() {
        assert_eq!(
            parse_theme("catppuccin-mocha"),
            (Palette::Catppuccin, ThemeMode::Dark)
        );
        assert_eq!(
            parse_theme("Catppuccin-Latte"),
            (Palette::Catppuccin, ThemeMode::Light)
        );
        assert_eq!(
            parse_theme("gruvbox-dark"),
            (Palette::Gruvbox, ThemeMode::Dark)
        );
        assert_eq!(parse_theme("nord"), (Palette::Nord, ThemeMode::Dark));
        assert_eq!(
            parse_theme("tokyonight"),
            (Palette::TokyoNight, ThemeMode::Dark)
        );
        assert_eq!(parse_theme("flexoki"), (Palette::Flexoki, ThemeMode::Dark));
        // Legacy mode names keep their old meaning (Flexoki + mode).
        assert_eq!(parse_theme("dark"), (Palette::Flexoki, ThemeMode::Dark));
        assert_eq!(parse_theme("auto"), (Palette::Flexoki, ThemeMode::Auto));
        // Unknown falls back to the default (Flexoki dark): a typo never
        // breaks the TUI.
        assert_eq!(parse_theme("banana"), (Palette::Flexoki, ThemeMode::Dark));
    }

    #[test]
    fn palette_cycle_wraps_through_all_families() {
        let mut p = Palette::Flexoki;
        let mut seen = vec![p.name()];
        for _ in 0..5 {
            p = p.cycle();
            seen.push(p.name());
        }
        assert_eq!(
            seen,
            vec![
                "flexoki",
                "catppuccin",
                "gruvbox",
                "nord",
                "tokyonight",
                "flexoki"
            ]
        );
    }

    #[test]
    fn preset_palettes_are_distinct_from_flexoki() {
        let base = Theme::dark();
        for (name, t) in [
            ("catppuccin-mocha", Theme::catppuccin_mocha()),
            ("gruvbox-dark", Theme::gruvbox_dark()),
            ("nord", Theme::nord()),
            ("tokyonight", Theme::tokyonight()),
        ] {
            assert_ne!(t.file_header, base.file_header, "{name} header differs");
            assert_ne!(t.status_bg, base.status_bg, "{name} status bg differs");
            assert_ne!(t.add, base.add, "{name} add color differs");
        }
        // Latte differs from Flexoki light too.
        assert_ne!(
            Theme::catppuccin_latte().status_bg,
            Theme::light().status_bg
        );
    }

    #[test]
    fn catppuccin_mode_resolves_to_its_own_variants() {
        let mocha = Palette::Catppuccin.theme(ThemeMode::Dark);
        let latte = Palette::Catppuccin.theme(ThemeMode::Light);
        assert_eq!(mocha.status_bg, Theme::catppuccin_mocha().status_bg);
        assert_eq!(latte.status_bg, Theme::catppuccin_latte().status_bg);
        // Single-variant families keep their dark theme in light mode.
        assert_eq!(
            Palette::Nord.theme(ThemeMode::Light).status_bg,
            Theme::nord().status_bg
        );
    }

    #[test]
    fn preset_names_read_like_config_values() {
        assert_eq!(
            Palette::Catppuccin.preset_name(ThemeMode::Dark),
            "catppuccin-mocha"
        );
        assert_eq!(
            Palette::Catppuccin.preset_name(ThemeMode::Light),
            "catppuccin-latte"
        );
        assert_eq!(Palette::Flexoki.preset_name(ThemeMode::Dark), "flexoki");
        assert_eq!(Palette::Nord.preset_name(ThemeMode::Dark), "nord");
        // Round-trip: a preset name parses back to the same (palette, mode).
        for (p, m) in [
            (Palette::Flexoki, ThemeMode::Dark),
            (Palette::Catppuccin, ThemeMode::Dark),
            (Palette::Catppuccin, ThemeMode::Light),
            (Palette::Gruvbox, ThemeMode::Dark),
            (Palette::Nord, ThemeMode::Dark),
            (Palette::TokyoNight, ThemeMode::Dark),
        ] {
            let name = p.preset_name(m);
            assert_eq!(parse_theme(&name), (p, m), "round-trip {name}");
        }
    }

    #[test]
    fn syntect_names_are_valid_builtin_selections() {
        for p in [
            Palette::Flexoki,
            Palette::Catppuccin,
            Palette::Gruvbox,
            Palette::Nord,
            Palette::TokyoNight,
        ] {
            for m in [ThemeMode::Dark, ThemeMode::Light] {
                let name = p.syntect_theme_name(m);
                assert!(
                    [
                        "base16-ocean.dark",
                        "base16-ocean.light",
                        "base16-mocha.dark",
                        "base16-eighties.dark",
                        "InspiredGitHub"
                    ]
                    .contains(&name),
                    "unexpected syntect pick {name} for {} {m:?}",
                    p.name()
                );
            }
        }
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
    fn on_accent_reads_over_every_accent_fill() {
        use super::test_support::contrast;
        for (name, t) in [
            ("flexoki-dark", Theme::dark()),
            ("flexoki-light", Theme::light()),
            ("catppuccin-mocha", Theme::catppuccin_mocha()),
            ("catppuccin-latte", Theme::catppuccin_latte()),
            ("gruvbox-dark", Theme::gruvbox_dark()),
            ("nord", Theme::nord()),
            ("tokyonight", Theme::tokyonight()),
        ] {
            for (slot, bg) in [
                ("add", t.add),
                ("delete", t.delete),
                ("hunk_header", t.hunk_header),
            ] {
                let ratio = contrast(t.on_accent, bg);
                assert!(ratio >= 3.0, "{name}: on_accent over {slot} = {ratio:.2}");
            }
            // The gold match fill keeps its ink pairing (match_active_fg).
            let gold = contrast(t.match_active_fg, t.match_active_bg);
            assert!(gold >= 3.0, "{name}: match fg over gold = {gold:.2}");
        }
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
        // Unknown / empty falls back to the default (Dark).
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
}
