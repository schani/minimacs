//! Follows the active Omarchy theme the way Neovim does.
//!
//! An Omarchy theme picks a Neovim colorscheme in its `neovim.lua`, and most
//! of those carry colors of their own that appear nowhere else in the theme.
//! So minimacs reads the same file and applies the same colorscheme:
//!
//! - Stock colorschemes come from `nvim_schemes.rs`, a generated snapshot of
//!   their resolved highlight groups.
//! - `aether`, Omarchy's generic scheme, is computed from the theme's
//!   `colors.toml`, as is any colorscheme minimacs has no snapshot for.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ratatui::style::{Color, Modifier, Style};

use super::nvim_schemes::SCHEMES;
use super::{Theme, HIGHLIGHT_NAMES};

/// The `face()` spelling of "this color is unset".
pub(super) const NONE: i64 = -1;

const BOLD: u8 = 1;
const ITALIC: u8 = 2;
const UNDERLINE: u8 = 4;
const REVERSE: u8 = 8;
const STRIKETHROUGH: u8 = 16;

/// A resolved Neovim highlight group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Face {
    fg: Option<u32>,
    bg: Option<u32>,
    attrs: u8,
}

pub(super) const fn face(fg: i64, bg: i64, attrs: u8) -> Face {
    Face {
        fg: if fg == NONE { None } else { Some(fg as u32) },
        bg: if bg == NONE { None } else { Some(bg as u32) },
        attrs,
    }
}

pub(super) struct NvimScheme {
    pub(super) name: &'static str,
    pub(super) groups: &'static [(&'static str, Face)],
}

/// Where Omarchy keeps the applied copy of the current theme.
pub(crate) fn current_theme_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/state/omarchy/current/theme"))
}

/// The theme for an Omarchy theme directory, or `None` if the directory
/// doesn't describe one.
pub(crate) fn load_theme(dir: &Path) -> Option<Theme> {
    let neovim_lua = std::fs::read_to_string(dir.join("neovim.lua")).unwrap_or_default();
    let snapshot = active_colorscheme(&neovim_lua)
        .and_then(|name| SCHEMES.iter().find(|scheme| scheme.name == name));
    if let Some(scheme) = snapshot {
        return Some(theme_from_groups(|group| lookup(scheme.groups, group)));
    }
    let colors_toml = std::fs::read_to_string(dir.join("colors.toml")).unwrap_or_default();
    // Omarchy resolves a theme's missing colors while generating neovim.lua,
    // so that palette, when there is one, is exactly what Neovim gets.
    let mut colors = parse_colors(&colors_toml);
    colors.extend(parse_colors(&neovim_lua));
    let groups = aether_groups(&colors)?;
    Some(theme_from_groups(|group| lookup(&groups, group)))
}

/// The value of the first uncommented `colorscheme = "name"` in a theme's
/// `neovim.lua`.
fn active_colorscheme(neovim_lua: &str) -> Option<&str> {
    neovim_lua
        .lines()
        .map(|line| line.split("--").next().unwrap_or_default())
        .find_map(|code| {
            let (_, after) = code.split_once("colorscheme")?;
            let value = after.trim_start().strip_prefix('=')?.trim_start();
            let quote = value.chars().next().filter(|c| matches!(c, '"' | '\''))?;
            value[1..].split_once(quote).map(|(name, _)| name)
        })
}

/// The `key = "#rrggbb"` entries of a `colors.toml` or generated `neovim.lua`.
fn parse_colors(source: &str) -> HashMap<&str, u32> {
    source
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            let hex = value
                .trim()
                .trim_end_matches(',')
                .trim_matches('"')
                .strip_prefix('#')?;
            Some((key.trim(), u32::from_str_radix(hex, 16).ok()?))
        })
        .collect()
}

/// The UI highlight groups a theme is built from.
#[cfg(test)]
const UI_GROUPS: &[&str] = &[
    "Normal",
    "StatusLine",
    "StatusLineNC",
    "Visual",
    "Search",
    "IncSearch",
    "WinSeparator",
    "NonText",
    "Pmenu",
];

/// The Neovim group that colors a capture from `HIGHLIGHT_NAMES`.
fn nvim_group(name: &str) -> &'static str {
    match name {
        "attribute" => "@attribute",
        "comment" => "@comment",
        "comment.documentation" => "@comment.documentation",
        "constant" => "@constant",
        "constant.builtin" => "@constant.builtin",
        "constructor" => "@constructor",
        "escape" => "@string.escape",
        "function" => "@function",
        "function.builtin" => "@function.builtin",
        "function.macro" => "@function.macro",
        "function.method" => "@function.method",
        "keyword" => "@keyword",
        "label" => "@label",
        "number" => "@number",
        "operator" => "@operator",
        "property" => "@property",
        "punctuation" | "punctuation.delimiter" => "@punctuation.delimiter",
        "punctuation.bracket" => "@punctuation.bracket",
        "punctuation.special" => "@punctuation.special",
        "string" => "@string",
        "string.special" => "@string.special",
        "tag" => "@tag",
        "text.emphasis" => "@markup.italic",
        "text.literal" => "@markup.raw",
        "text.reference" => "@markup.link.label",
        "text.strong" => "@markup.strong",
        "text.title" | "markup.heading" => "@markup.heading",
        "text.uri" => "@markup.link.url",
        "type" => "@type",
        "type.builtin" => "@type.builtin",
        "variable" => "@variable",
        "variable.builtin" => "@variable.builtin",
        "variable.parameter" => "@variable.parameter",
        "markup.link" => "@markup.link",
        other => unreachable!("no Neovim group for capture {other:?}"),
    }
}

fn lookup(groups: &[(&str, Face)], group: &str) -> Face {
    groups
        .iter()
        .find(|(name, _)| *name == group)
        .map(|(_, face)| *face)
        .unwrap_or_default()
}

fn color(value: u32) -> Color {
    Color::Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

impl Face {
    fn style(self) -> Style {
        let mut style = Style::default();
        if let Some(fg) = self.fg {
            style = style.fg(color(fg));
        }
        if let Some(bg) = self.bg {
            style = style.bg(color(bg));
        }
        for (attr, modifier) in [
            (BOLD, Modifier::BOLD),
            (ITALIC, Modifier::ITALIC),
            (UNDERLINE, Modifier::UNDERLINED),
            (REVERSE, Modifier::REVERSED),
            (STRIKETHROUGH, Modifier::CROSSED_OUT),
        ] {
            if self.attrs & attr != 0 {
                style = style.add_modifier(modifier);
            }
        }
        style
    }

    /// Omarchy runs Neovim with a transparent background, so plain text and
    /// the bars drawn over it leave the terminal's background alone.
    fn transparent(self) -> Self {
        Self { bg: None, ..self }
    }
}

fn theme_from_groups(lookup: impl Fn(&str) -> Face) -> Theme {
    let ui = |group: &str| lookup(group).style();
    let mode_line_unfocused = ui("StatusLineNC");
    let mut mode_line_focused = ui("StatusLine");
    if mode_line_focused == mode_line_unfocused {
        mode_line_focused = mode_line_focused.add_modifier(Modifier::BOLD);
    }
    Theme {
        text: Face {
            attrs: 0,
            ..lookup("Normal").transparent()
        }
        .style(),
        mode_line_focused,
        mode_line_unfocused,
        region: ui("Visual"),
        search_current: ui("IncSearch"),
        search_other: ui("Search"),
        separator: lookup("WinSeparator").transparent().style(),
        fringe: lookup("NonText").transparent().style(),
        completions: ui("Pmenu"),
        highlights: HIGHLIGHT_NAMES
            .iter()
            .map(|name| lookup(nvim_group(name)).style())
            .collect(),
    }
}

/// aether.nvim's `Util.blend`: `fg` over `bg` at opacity `alpha`.
fn blend(fg: u32, alpha: f64, bg: u32) -> u32 {
    [16, 8, 0].iter().fold(0, |blended, shift| {
        let channel = |value: u32| f64::from((value >> shift) & 0xFF);
        let mixed = alpha * channel(fg) + (1.0 - alpha) * channel(bg);
        blended | ((mixed.clamp(0.0, 255.0) + 0.5).floor() as u32) << shift
    })
}

/// The groups aether.nvim, Omarchy's generic colorscheme, derives from a
/// theme's palette. `None` if the palette is missing a color it needs.
fn aether_groups(colors: &HashMap<&str, u32>) -> Option<Vec<(&'static str, Face)>> {
    let get = |key: &str| colors.get(key).map(|value| i64::from(*value));
    // neovim.lua abbreviates three of the names colors.toml spells out.
    let bg = get("bg").or_else(|| get("background"))?;
    let fg = get("fg").or_else(|| get("foreground"))?;
    let dark_bg = get("dark_bg").or_else(|| get("dark_background"))?;
    let muted = get("muted")?;
    let selection = get("selection")?;
    let yellow = get("yellow")?;
    let orange = get("orange").unwrap_or(yellow);
    let green = get("green")?;
    let cyan = get("cyan")?;
    let blue = get("blue")?;
    let bright_red = get("bright_red")?;
    let bright_yellow = get("bright_yellow")?;
    let bright_cyan = get("bright_cyan")?;
    let bright_blue = get("bright_blue")?;
    let bright_purple = get("bright_magenta")?;

    let blend_bg = |color: i64, alpha: f64| i64::from(blend(color as u32, alpha, bg as u32));
    let plain = |color: i64| face(color, NONE, 0);
    let punctuation = plain(blend_bg(fg, 0.5));

    Some(vec![
        ("Normal", face(fg, bg, 0)),
        ("StatusLine", face(fg, dark_bg, 0)),
        ("StatusLineNC", face(muted, dark_bg, 0)),
        ("Visual", face(NONE, selection, 0)),
        ("Search", face(fg, blend_bg(blue, 0.4), BOLD)),
        ("IncSearch", face(bg, orange, BOLD)),
        ("WinSeparator", face(muted, NONE, BOLD)),
        ("NonText", plain(muted)),
        ("Pmenu", face(fg, dark_bg, 0)),
        ("@attribute", plain(cyan)),
        ("@comment", face(muted, NONE, ITALIC)),
        ("@comment.documentation", face(muted, NONE, ITALIC)),
        ("@constant", plain(bright_yellow)),
        ("@constant.builtin", plain(cyan)),
        ("@constructor", plain(yellow)),
        ("@string.escape", plain(bright_purple)),
        ("@function", face(blue, NONE, BOLD)),
        ("@function.builtin", plain(cyan)),
        ("@function.macro", plain(cyan)),
        ("@function.method", face(blue, NONE, BOLD)),
        ("@keyword", face(bright_purple, NONE, ITALIC)),
        ("@label", plain(yellow)),
        ("@number", plain(orange)),
        ("@operator", plain(fg)),
        ("@property", plain(bright_cyan)),
        ("@punctuation.delimiter", punctuation),
        ("@punctuation.bracket", punctuation),
        ("@punctuation.special", plain(bright_blue)),
        ("@string", plain(green)),
        ("@string.special", plain(bright_red)),
        ("@tag", plain(yellow)),
        ("@markup.italic", face(NONE, NONE, ITALIC)),
        ("@markup.raw", plain(green)),
        ("@markup.link.label", plain(blue)),
        ("@markup.strong", face(NONE, NONE, BOLD)),
        ("@markup.heading", face(blue, NONE, BOLD)),
        ("@markup.link.url", face(cyan, NONE, UNDERLINE)),
        ("@markup.link", face(blue, NONE, UNDERLINE)),
        ("@type", face(yellow, NONE, BOLD)),
        ("@type.builtin", plain(fg)),
        ("@variable", plain(fg)),
        ("@variable.builtin", face(fg, NONE, BOLD)),
        ("@variable.parameter", plain(cyan)),
    ])
}

#[cfg(test)]
mod tests {
    use super::super::nvim_schemes::{AETHER_RISTRETTO_COLORS, AETHER_RISTRETTO_FIXTURE};
    use super::*;

    fn rgb(value: u32) -> Color {
        Color::Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
    }

    fn highlight(theme: &Theme, name: &str) -> Style {
        let idx = HIGHLIGHT_NAMES.iter().position(|n| *n == name).unwrap();
        theme.highlight(idx)
    }

    fn theme_dir(neovim_lua: Option<&str>, colors_toml: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        if let Some(contents) = neovim_lua {
            std::fs::write(dir.path().join("neovim.lua"), contents).unwrap();
        }
        if let Some(contents) = colors_toml {
            std::fs::write(dir.path().join("colors.toml"), contents).unwrap();
        }
        dir
    }

    const MATTE_BLACK_LUA: &str = r#"return {
  { "tahayvr/matteblack.nvim", lazy = false, priority = 1000 },
  {
		"LazyVim/LazyVim",
		opts = {
			colorscheme = "matteblack",
		},
	},
}"#;

    #[test]
    fn finds_the_colorscheme_a_theme_selects() {
        assert_eq!(active_colorscheme(MATTE_BLACK_LUA), Some("matteblack"));
        assert_eq!(
            active_colorscheme("opts = { colorscheme='rose-pine-dawn' }"),
            Some("rose-pine-dawn")
        );
        assert_eq!(active_colorscheme("return {}"), None);
    }

    #[test]
    fn ignores_commented_out_colorschemes() {
        let lua = "-- colorscheme = \"old\"\nopts = { colorscheme = \"new\" }\n";
        assert_eq!(active_colorscheme(lua), Some("new"));
    }

    #[test]
    fn parses_only_hex_colors_from_colors_toml() {
        let colors =
            parse_colors("mode = \"dark\"\n\n# note\nblue = \"#e68e0d\"\nred=\"#D35F5F\"\n");
        assert_eq!(colors.get("blue"), Some(&0xE68E0D));
        assert_eq!(colors.get("red"), Some(&0xD35F5F));
        assert_eq!(colors.get("mode"), None);
    }

    #[test]
    fn matte_black_uses_its_neovim_colorscheme_not_the_shared_palette() {
        // colors.toml has no #8A8A8D or #059669; only matteblack.nvim does.
        let dir = theme_dir(Some(MATTE_BLACK_LUA), Some("blue = \"#e68e0d\"\n"));
        let theme = load_theme(dir.path()).unwrap();

        let comment = highlight(&theme, "comment");
        assert_eq!(comment.fg, Some(rgb(0x8A8A8D)));
        assert!(comment.add_modifier.contains(Modifier::ITALIC));
        assert_eq!(highlight(&theme, "keyword").fg, Some(rgb(0x059669)));
        assert_eq!(highlight(&theme, "type").fg, Some(rgb(0xFBBF24)));
        assert_eq!(highlight(&theme, "function").fg, Some(rgb(0xDC2626)));
        assert_eq!(highlight(&theme, "function.method").fg, Some(rgb(0xF59E0B)));
        assert_eq!(theme.mode_line_focused.bg, Some(rgb(0x212121)));
        assert_eq!(theme.mode_line_unfocused.bg, Some(rgb(0x333333)));
        assert_eq!(theme.region.bg, Some(rgb(0x333333)));
        assert_eq!(theme.search_current.bg, Some(rgb(0xEFBF04)));
        assert_eq!(theme.search_other.bg, Some(rgb(0xF59E0B)));
        assert_eq!(theme.completions.bg, Some(rgb(0x212121)));
    }

    #[test]
    fn text_keeps_the_terminal_background_like_transparent_neovim() {
        let dir = theme_dir(Some(MATTE_BLACK_LUA), None);
        let theme = load_theme(dir.path()).unwrap();
        assert_eq!(theme.text.fg, Some(rgb(0xEAEAEA)));
        assert_eq!(theme.text.bg, None);
        assert_eq!(theme.separator.bg, None);
        assert_eq!(theme.fringe.bg, None);
    }

    #[test]
    fn every_snapshot_covers_every_group_minimacs_asks_for() {
        for scheme in SCHEMES {
            for name in HIGHLIGHT_NAMES
                .iter()
                .copied()
                .map(nvim_group)
                .chain(UI_GROUPS.iter().copied())
            {
                assert!(
                    scheme.groups.iter().any(|(group, _)| *group == name),
                    "{} lacks {name}",
                    scheme.name
                );
            }
        }
    }

    #[test]
    fn every_snapshot_tells_focused_panes_apart() {
        for scheme in SCHEMES {
            let theme = theme_from_groups(|group| lookup(scheme.groups, group));
            assert_ne!(
                theme.mode_line_focused, theme.mode_line_unfocused,
                "{}",
                scheme.name
            );
        }
    }

    #[test]
    fn aether_port_matches_the_real_plugin() {
        let colors = parse_colors(AETHER_RISTRETTO_COLORS);
        let groups = aether_groups(&colors).unwrap();
        for (group, expected) in AETHER_RISTRETTO_FIXTURE {
            assert_eq!(lookup(&groups, group), *expected, "{group}");
        }
    }

    #[test]
    fn aether_and_unknown_colorschemes_are_built_from_colors_toml() {
        for lua in [
            "opts = { colorscheme = \"aether\" }",
            "opts = { colorscheme = \"somebodys-custom-scheme\" }",
        ] {
            let dir = theme_dir(Some(lua), Some(AETHER_RISTRETTO_COLORS));
            let theme = load_theme(dir.path()).unwrap();
            let colors = parse_colors(AETHER_RISTRETTO_COLORS);
            assert_eq!(highlight(&theme, "string").fg, Some(rgb(colors["green"])));
        }
    }

    #[test]
    fn prefers_the_resolved_palette_omarchy_generated_for_neovim() {
        // What neovim.lua.tpl renders: aether's names, every gap filled in.
        let lua = r##"opts = {
      colors = {
        bg = "#101010",
        dark_bg = "#0a0a0a",
        fg = "#d0d0d0",
        muted = "#606060",
        red = "#aa0000",
        yellow = "#aaaa00",
        orange = "#ff8800",
        green = "#00aa00",
        cyan = "#00aaaa",
        blue = "#0000aa",
        bright_red = "#ff0000",
        bright_yellow = "#ffff00",
        bright_cyan = "#00ffff",
        bright_blue = "#0000ff",
        bright_magenta = "#ff00ff",
        selection = "#303030",
      },
    },
    colorscheme = "aether","##;
        let dir = theme_dir(Some(lua), Some("orange = \"#123456\"\n"));
        let theme = load_theme(dir.path()).unwrap();
        assert_eq!(highlight(&theme, "number").fg, Some(rgb(0xFF8800)));
        assert_eq!(theme.text.fg, Some(rgb(0xD0D0D0)));
    }

    #[test]
    fn a_palette_without_orange_falls_back_to_yellow_like_omarchy() {
        let without_orange: String = AETHER_RISTRETTO_COLORS
            .lines()
            .filter(|line| !line.starts_with("orange"))
            .map(|line| format!("{line}\n"))
            .collect();
        let dir = theme_dir(None, Some(&without_orange));
        let theme = load_theme(dir.path()).unwrap();
        let colors = parse_colors(AETHER_RISTRETTO_COLORS);
        assert_eq!(highlight(&theme, "number").fg, Some(rgb(colors["yellow"])));
    }

    #[test]
    fn a_theme_without_neovim_lua_still_uses_colors_toml() {
        let dir = theme_dir(None, Some(AETHER_RISTRETTO_COLORS));
        assert!(load_theme(dir.path()).is_some());
    }

    #[test]
    fn missing_or_incomplete_themes_are_rejected() {
        let empty = theme_dir(None, None);
        assert_eq!(load_theme(empty.path()), None);
        assert_eq!(load_theme(Path::new("/nonexistent/theme")), None);
        let partial = theme_dir(
            Some("colorscheme = \"aether\""),
            Some("blue = \"#112233\"\n"),
        );
        assert_eq!(load_theme(partial.path()), None);
    }

    #[test]
    fn blending_matches_aether() {
        assert_eq!(blend(0xFFFFFF, 0.5, 0x000000), 0x808080);
        assert_eq!(blend(0x102030, 1.0, 0xABCDEF), 0x102030);
        assert_eq!(blend(0x102030, 0.0, 0xABCDEF), 0xABCDEF);
    }
}
