//! Generates a theme from the active Omarchy theme's palette.
//!
//! `colors.toml` is the only input. The color scheme is derived from it with
//! the algorithm of aether, the scheme Omarchy generates for Neovim, so
//! captures and UI roles get the colors aether would give them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ratatui::style::{Color, Modifier, Style};

use super::{Theme, HIGHLIGHT_NAMES};

/// The `face()` spelling of "this color is unset".
pub(super) const NONE: i64 = -1;

const BOLD: u8 = 1;
const ITALIC: u8 = 2;
const UNDERLINE: u8 = 4;

/// One highlight group of the generated scheme.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Face {
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

/// Where Omarchy keeps the applied copy of the current theme.
pub(crate) fn current_theme_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/state/omarchy/current/theme"))
}

/// The theme for an Omarchy theme directory, or `None` if it has no usable
/// `colors.toml`.
pub(crate) fn load_theme(dir: &Path) -> Option<Theme> {
    let colors_toml = std::fs::read_to_string(dir.join("colors.toml")).ok()?;
    let colors = resolve_palette(parse_colors(&colors_toml));
    let mut groups = aether_groups(&colors)?;
    raise_contrast(&mut groups, colors["background"], colors["foreground"]);
    Some(theme_from_groups(|group| lookup(&groups, group)))
}

/// The `key = "#rrggbb"` entries of a theme's flat `colors.toml`.
fn parse_colors(colors_toml: &str) -> HashMap<&str, u32> {
    colors_toml
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            let hex = value.trim().trim_matches('"').strip_prefix('#')?;
            Some((key.trim(), u32::from_str_radix(hex, 16).ok()?))
        })
        .collect()
}

/// The aether group that colors a capture from `HIGHLIGHT_NAMES`.
fn aether_group(name: &str) -> &'static str {
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
        other => unreachable!("no aether group for capture {other:?}"),
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
        ] {
            if self.attrs & attr != 0 {
                style = style.add_modifier(modifier);
            }
        }
        style
    }

    /// Plain text and the bars drawn over it leave the terminal's background
    /// alone, so translucent terminals stay translucent.
    fn transparent(self) -> Self {
        Self { bg: None, ..self }
    }
}

fn theme_from_groups(lookup: impl Fn(&str) -> Face) -> Theme {
    let ui = |group: &str| lookup(group).style();
    Theme {
        text: Face {
            attrs: 0,
            ..lookup("Normal").transparent()
        }
        .style(),
        mode_line_focused: ui("StatusLine"),
        mode_line_unfocused: ui("StatusLineNC"),
        region: ui("Visual"),
        search_current: ui("IncSearch"),
        search_other: ui("Search"),
        separator: lookup("WinSeparator").transparent().style(),
        fringe: lookup("NonText").transparent().style(),
        completions: ui("Pmenu"),
        highlights: HIGHLIGHT_NAMES
            .iter()
            .map(|name| lookup(aether_group(name)).style())
            .collect(),
    }
}

/// aether's `Util.blend`, which is also Omarchy's `mix_color`: `fg` over `bg`
/// at opacity `alpha`.
fn blend(fg: u32, alpha: f64, bg: u32) -> u32 {
    [16, 8, 0].iter().fold(0, |blended, shift| {
        let channel = |value: u32| f64::from((value >> shift) & 0xFF);
        let mixed = alpha * channel(fg) + (1.0 - alpha) * channel(bg);
        blended | ((mixed.clamp(0.0, 255.0) + 0.5).floor() as u32) << shift
    })
}

/// The lowest WCAG contrast ratio generated text may have. Many palettes'
/// faintest colors, `muted` above all, fall well below it.
const MIN_CONTRAST: f64 = 3.0;

/// WCAG relative luminance of an `0xRRGGBB` color.
fn relative_luminance(color: u32) -> f64 {
    let channel = |shift: u32| {
        let value = f64::from((color >> shift) & 0xFF) / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

/// WCAG contrast ratio, from 1 (identical) to 21 (black on white).
fn contrast_ratio(a: u32, b: u32) -> f64 {
    let (a, b) = (relative_luminance(a), relative_luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Blend each too-faint text color toward the theme's foreground, stopping at
/// the first step that reaches `MIN_CONTRAST` against what the text sits on:
/// the group's own background, or else the theme's. Colors already readable
/// don't change. Where the foreground can't reach the minimum, as with text
/// on a mid-toned bar, the color moves away from that background instead;
/// one of white and black always reaches it. Separators are lines, not text,
/// and stay quiet.
fn raise_contrast(groups: &mut [(&str, Face)], background: u32, foreground: u32) {
    for (group, face) in groups.iter_mut() {
        if *group == "WinSeparator" {
            continue;
        }
        let Some(color) = face.fg else {
            continue;
        };
        let on = face.bg.unwrap_or(background);
        let (away, opposite) = if relative_luminance(color) > relative_luminance(on) {
            (0xFFFFFF, 0x000000)
        } else {
            (0x000000, 0xFFFFFF)
        };
        face.fg = [foreground, away, opposite]
            .into_iter()
            .find_map(|target| {
                (0..=100)
                    .map(|step| blend(target, f64::from(step) / 100.0, color))
                    .find(|candidate| contrast_ratio(*candidate, on) >= MIN_CONTRAST)
            })
            .or(Some(color));
    }
}

/// Omarchy's rules for colors a theme leaves out, in order: a missing name
/// takes the first available source (see `omarchy-theme-color`).
const PALETTE_ALIASES: &[(&str, &str)] = &[
    ("background", "bg"),
    ("dark_background", "dark_bg"),
    ("foreground", "fg"),
    ("dark_foreground", "dark_fg"),
    ("background", "color0"),
    ("foreground", "color7"),
    ("red", "color1"),
    ("green", "color2"),
    ("yellow", "color3"),
    ("blue", "color4"),
    ("magenta", "color5"),
    ("cyan", "color6"),
    ("bright_red", "color9"),
    ("bright_yellow", "color11"),
    ("bright_blue", "color12"),
    ("bright_magenta", "color13"),
    ("bright_cyan", "color14"),
    ("magenta", "purple"),
    ("bright_magenta", "bright_purple"),
    ("dark_foreground", "color8"),
    ("dark_foreground", "foreground"),
    ("muted", "color8"),
    ("muted", "dark_foreground"),
    ("selection", "selection_background"),
    ("selection", "color8"),
    ("selection", "background"),
    ("orange", "yellow"),
];

/// Shades Omarchy derives when still missing: `(name, base, mixed_with, amount)`.
const PALETTE_SHADES: &[(&str, &str, u32, f64)] = &[
    ("dark_background", "background", 0x000000, 0.25),
    ("bright_red", "red", 0xFFFFFF, 0.2),
    ("bright_yellow", "yellow", 0xFFFFFF, 0.2),
    ("bright_blue", "blue", 0xFFFFFF, 0.2),
    ("bright_magenta", "magenta", 0xFFFFFF, 0.2),
    ("bright_cyan", "cyan", 0xFFFFFF, 0.2),
];

/// `colors` with the gaps filled in the way Omarchy fills them.
fn resolve_palette(mut colors: HashMap<&str, u32>) -> HashMap<&str, u32> {
    for (name, source) in PALETTE_ALIASES {
        if !colors.contains_key(name) {
            if let Some(value) = colors.get(source).copied() {
                colors.insert(name, value);
            }
        }
    }
    for (name, base, mixed_with, amount) in PALETTE_SHADES {
        if !colors.contains_key(name) {
            if let Some(value) = colors.get(base).copied() {
                colors.insert(name, blend(*mixed_with, *amount, value));
            }
        }
    }
    colors
}

/// The groups aether derives from a resolved palette. `None` if a color it
/// needs is missing.
fn aether_groups(colors: &HashMap<&str, u32>) -> Option<Vec<(&'static str, Face)>> {
    let get = |key: &str| colors.get(key).map(|value| i64::from(*value));
    let bg = get("background")?;
    let fg = get("foreground")?;
    let dark_bg = get("dark_background")?;
    let muted = get("muted")?;
    let selection = get("selection")?;
    let yellow = get("yellow")?;
    let orange = get("orange")?;
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

/// Notices when Omarchy applies a different theme, by the identity of its
/// `colors.toml`. Omarchy swaps the whole directory in place, so a missing
/// file only means a switch is underway.
pub(crate) struct ThemeWatcher {
    dir: PathBuf,
    stamp: Stamp,
}

type Stamp = Option<(SystemTime, u64)>;

impl ThemeWatcher {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir, stamp: None }
    }

    /// The theme, the first time it is seen and after each change. `None`
    /// means there is nothing new to apply.
    pub(crate) fn load_if_changed(&mut self) -> Option<Theme> {
        let stamp = std::fs::metadata(self.dir.join("colors.toml"))
            .ok()
            .and_then(|metadata| Some((metadata.modified().ok()?, metadata.len())));
        if stamp == self.stamp {
            return None;
        }
        self.stamp = stamp;
        load_theme(&self.dir)
    }
}

#[cfg(test)]
mod tests {
    use super::super::aether_fixture::{AETHER_RISTRETTO_COLORS, AETHER_RISTRETTO_FIXTURE};
    use super::*;

    fn rgb(value: u32) -> Color {
        Color::Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
    }

    fn highlight(theme: &Theme, name: &str) -> Style {
        let idx = HIGHLIGHT_NAMES.iter().position(|n| *n == name).unwrap();
        theme.highlight(idx)
    }

    fn theme_dir(colors_toml: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("colors.toml"), colors_toml).unwrap();
        dir
    }

    fn ristretto() -> HashMap<&'static str, u32> {
        resolve_palette(parse_colors(AETHER_RISTRETTO_COLORS))
    }

    const MATTE_BLACK: &str = r##"
accent = "#e68e0d"
selection = "#2a2a2a"
muted = "#333333"
background = "#121212"
dark_background = "#0d0d0d"
foreground = "#bebebe"
red = "#D35F5F"
yellow = "#b91c1c"
orange = "#c63d3d"
green = "#FFC107"
cyan = "#bebebe"
blue = "#e68e0d"
magenta = "#D35F5F"
bright_red = "#B91C1C"
bright_yellow = "#b90a0a"
bright_cyan = "#eaeaea"
bright_blue = "#f59e0b"
bright_magenta = "#B91C1C"
"##;

    #[test]
    fn parses_only_hex_colors_from_colors_toml() {
        let colors =
            parse_colors("mode = \"dark\"\n\n# note\nblue = \"#e68e0d\"\nred=\"#D35F5F\"\n");
        assert_eq!(colors.get("blue"), Some(&0xE68E0D));
        assert_eq!(colors.get("red"), Some(&0xD35F5F));
        assert_eq!(colors.get("mode"), None);
    }

    #[test]
    fn generated_scheme_matches_the_real_aether_plugin() {
        let groups = aether_groups(&ristretto()).unwrap();
        for (group, expected) in AETHER_RISTRETTO_FIXTURE {
            assert_eq!(lookup(&groups, group), *expected, "{group}");
        }
    }

    #[test]
    fn matte_black_is_generated_from_its_palette() {
        let dir = theme_dir(MATTE_BLACK);
        let theme = load_theme(dir.path()).unwrap();

        let comment = highlight(&theme, "comment");
        assert!(comment.add_modifier.contains(Modifier::ITALIC));
        let keyword = highlight(&theme, "keyword");
        assert!(keyword.add_modifier.contains(Modifier::ITALIC));
        let function = highlight(&theme, "function");
        assert_eq!(function.fg, Some(rgb(0xE68E0D)));
        assert!(function.add_modifier.contains(Modifier::BOLD));
        assert_eq!(highlight(&theme, "string").fg, Some(rgb(0xFFC107)));
        assert_eq!(theme.mode_line_focused.bg, Some(rgb(0x0D0D0D)));
        assert_eq!(theme.region.bg, Some(rgb(0x2A2A2A)));
        assert_eq!(theme.region.fg, None, "selection keeps syntax colors");
    }

    const ROSE_PINE: &str = r##"
selection = "#dfdad9"
muted = "#cecacd"
background = "#faf4ed"
dark_background = "#ede7e1"
foreground = "#575279"
red = "#b4637a"
yellow = "#ea9d34"
orange = "#cf8057"
green = "#286983"
cyan = "#d7827e"
blue = "#56949f"
magenta = "#907aa9"
bright_red = "#b4637a"
bright_yellow = "#ea9d34"
bright_cyan = "#d7827e"
bright_blue = "#56949f"
bright_magenta = "#907aa9"
"##;

    #[test]
    fn contrast_ratio_follows_wcag() {
        assert!((contrast_ratio(0x000000, 0xFFFFFF) - 21.0).abs() < 0.001);
        assert!((contrast_ratio(0xFFFFFF, 0x000000) - 21.0).abs() < 0.001);
        assert!((contrast_ratio(0x777777, 0x777777) - 1.0).abs() < 0.001);
    }

    #[test]
    fn faint_comments_are_raised_just_enough_to_read() {
        let dir = theme_dir(ROSE_PINE);
        let theme = load_theme(dir.path()).unwrap();
        // muted (#cecacd) is 1.5:1 on the background; this is the first
        // step toward the foreground that reaches 3:1.
        assert_eq!(highlight(&theme, "comment").fg, Some(rgb(0x8F8AA0)));
    }

    #[test]
    fn every_generated_text_color_reaches_the_minimum() {
        for palette in [ROSE_PINE, MATTE_BLACK, AETHER_RISTRETTO_COLORS] {
            let colors = resolve_palette(parse_colors(palette));
            let mut groups = aether_groups(&colors).unwrap();
            raise_contrast(&mut groups, colors["background"], colors["foreground"]);
            for (group, face) in groups {
                let Some(fg) = face.fg.filter(|_| group != "WinSeparator") else {
                    continue;
                };
                let on = face.bg.unwrap_or(colors["background"]);
                assert!(
                    contrast_ratio(fg, on) >= MIN_CONTRAST,
                    "{group}: #{fg:06x} on #{on:06x}"
                );
            }
        }
    }

    #[test]
    fn colors_above_the_minimum_are_left_alone() {
        let readable = face(0x101010, NONE, ITALIC);
        let mut groups = vec![("@keyword", readable)];
        raise_contrast(&mut groups, 0xFFFFFF, 0x000000);
        assert_eq!(groups[0].1, readable);
    }

    #[test]
    fn raising_only_changes_the_foreground_and_keeps_attributes() {
        let mut groups = vec![("@comment", face(0xEEEEEE, NONE, ITALIC))];
        raise_contrast(&mut groups, 0xFFFFFF, 0x000000);
        assert_ne!(groups[0].1.fg, Some(0xEEEEEE));
        assert_eq!(groups[0].1.bg, None);
        assert_eq!(groups[0].1.attrs, ITALIC);
    }

    #[test]
    fn contrast_is_measured_against_the_groups_own_background() {
        // Fine on white, unreadable on its own near-identical bar.
        let mut groups = vec![("StatusLineNC", face(0x404040, 0x383838, 0))];
        raise_contrast(&mut groups, 0xFFFFFF, 0xF0F0F0);
        let raised = groups[0].1.fg.unwrap();
        assert!(contrast_ratio(raised, 0x383838) >= MIN_CONTRAST);
    }

    #[test]
    fn separators_and_groups_without_a_foreground_stay_quiet() {
        let separator = face(0xEEEEEE, NONE, BOLD);
        let selection = face(NONE, 0xEEEEEE, 0);
        let mut groups = vec![("WinSeparator", separator), ("Visual", selection)];
        raise_contrast(&mut groups, 0xFFFFFF, 0x000000);
        assert_eq!(
            groups,
            vec![("WinSeparator", separator), ("Visual", selection)]
        );
    }

    #[test]
    fn text_on_a_colored_bar_moves_away_from_it_when_the_foreground_cannot_help() {
        // Rose Pine's current search match: cream on orange, with a dark
        // foreground that only passes through worse contrast on the way.
        let mut groups = vec![("IncSearch", face(0xFAF4ED, 0xCF8057, BOLD))];
        raise_contrast(&mut groups, 0xFAF4ED, 0x575279);
        let raised = groups[0].1.fg.unwrap();
        assert!(contrast_ratio(raised, 0xCF8057) >= MIN_CONTRAST);
        assert!(relative_luminance(raised) > relative_luminance(0xCF8057));
    }

    #[test]
    fn the_minimum_is_reached_even_when_foreground_equals_background() {
        let mut groups = vec![("@comment", face(0x818181, NONE, 0))];
        raise_contrast(&mut groups, 0x808080, 0x808080);
        let raised = groups[0].1.fg.unwrap();
        assert!(contrast_ratio(raised, 0x808080) >= MIN_CONTRAST);
    }

    #[test]
    fn nothing_but_colors_toml_is_read() {
        let dir = theme_dir(AETHER_RISTRETTO_COLORS);
        std::fs::write(
            dir.path().join("neovim.lua"),
            "opts = { colorscheme = \"matteblack\", colors = { green = \"#123456\" } }",
        )
        .unwrap();
        let theme = load_theme(dir.path()).unwrap();
        assert_eq!(
            highlight(&theme, "string").fg,
            Some(rgb(ristretto()["green"]))
        );

        let only_nvim = tempfile::tempdir().unwrap();
        std::fs::write(
            only_nvim.path().join("neovim.lua"),
            "opts = { colorscheme = \"matteblack\" }",
        )
        .unwrap();
        assert_eq!(load_theme(only_nvim.path()), None);
    }

    #[test]
    fn text_keeps_the_terminal_background() {
        let dir = theme_dir(MATTE_BLACK);
        let theme = load_theme(dir.path()).unwrap();
        assert_eq!(theme.text.fg, Some(rgb(0xBEBEBE)));
        assert_eq!(theme.text.bg, None);
        assert_eq!(theme.separator.bg, None);
        assert_eq!(theme.fringe.bg, None);
    }

    #[test]
    fn focused_and_unfocused_mode_lines_differ() {
        let dir = theme_dir(MATTE_BLACK);
        let theme = load_theme(dir.path()).unwrap();
        assert_ne!(theme.mode_line_focused, theme.mode_line_unfocused);
    }

    #[test]
    fn a_palette_without_orange_falls_back_to_yellow_like_omarchy() {
        let without_orange: String = AETHER_RISTRETTO_COLORS
            .lines()
            .filter(|line| !line.starts_with("orange"))
            .map(|line| format!("{line}\n"))
            .collect();
        let colors = resolve_palette(parse_colors(&without_orange));
        assert_eq!(colors["orange"], colors["yellow"]);
    }

    /// Expected values are the output of Omarchy's own `omarchy-theme-color`.
    #[test]
    fn fills_gaps_in_a_legacy_ansi_palette_like_omarchy() {
        let legacy = r##"
color0 = "#101820"
color1 = "#a01010"
color2 = "#10a010"
color3 = "#a0a010"
color4 = "#1010a0"
color5 = "#a010a0"
color6 = "#10a0a0"
color7 = "#d0d0d0"
color8 = "#606060"
"##;
        let colors = resolve_palette(parse_colors(legacy));
        for (key, expected) in [
            ("background", 0x101820),
            ("foreground", 0xD0D0D0),
            ("dark_background", 0x0C1218),
            ("muted", 0x606060),
            ("selection", 0x606060),
            ("yellow", 0xA0A010),
            ("orange", 0xA0A010),
            ("green", 0x10A010),
            ("cyan", 0x10A0A0),
            ("blue", 0x1010A0),
            ("bright_red", 0xB34040),
            ("bright_yellow", 0xB3B340),
            ("bright_cyan", 0x40B3B3),
            ("bright_blue", 0x4040B3),
            ("bright_magenta", 0xB340B3),
        ] {
            assert_eq!(colors.get(key), Some(&expected), "{key}");
        }
        assert!(aether_groups(&colors).is_some());
    }

    #[test]
    fn accepts_short_palette_names() {
        let colors = resolve_palette(parse_colors(
            "bg = \"#101010\"\nfg = \"#202020\"\ndark_bg = \"#303030\"\n",
        ));
        assert_eq!(colors["background"], 0x101010);
        assert_eq!(colors["foreground"], 0x202020);
        assert_eq!(colors["dark_background"], 0x303030);
    }

    #[test]
    fn missing_or_incomplete_themes_are_rejected() {
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(load_theme(empty.path()), None);
        assert_eq!(load_theme(Path::new("/nonexistent/theme")), None);
        let partial = theme_dir("blue = \"#112233\"\n");
        assert_eq!(load_theme(partial.path()), None);
    }

    #[test]
    fn watcher_loads_the_theme_once_and_again_only_after_a_change() {
        let dir = theme_dir(MATTE_BLACK);
        let mut watcher = ThemeWatcher::new(dir.path().to_path_buf());

        let first = watcher.load_if_changed().unwrap();
        assert_eq!(highlight(&first, "string").fg, Some(rgb(0xFFC107)));
        assert_eq!(watcher.load_if_changed(), None);

        std::fs::write(dir.path().join("colors.toml"), AETHER_RISTRETTO_COLORS).unwrap();
        let second = watcher.load_if_changed().unwrap();
        assert_ne!(highlight(&second, "string"), highlight(&first, "string"));
    }

    #[test]
    fn watcher_survives_omarchy_replacing_the_theme_directory() {
        let parent = tempfile::tempdir().unwrap();
        let current = parent.path().join("theme");
        let next = parent.path().join("next-theme");
        std::fs::create_dir(&current).unwrap();
        std::fs::write(current.join("colors.toml"), MATTE_BLACK).unwrap();
        let mut watcher = ThemeWatcher::new(current.clone());
        assert!(watcher.load_if_changed().is_some());

        // omarchy-theme-set: rm -rf current, then mv next current.
        std::fs::remove_dir_all(&current).unwrap();
        assert_eq!(
            watcher.load_if_changed(),
            None,
            "mid-switch keeps the theme"
        );

        std::fs::create_dir(&next).unwrap();
        std::fs::write(next.join("colors.toml"), AETHER_RISTRETTO_COLORS).unwrap();
        std::fs::rename(&next, &current).unwrap();
        assert!(watcher.load_if_changed().is_some());
    }

    #[test]
    fn watcher_of_a_system_without_omarchy_never_loads() {
        let mut watcher = ThemeWatcher::new(PathBuf::from("/nonexistent/theme"));
        assert_eq!(watcher.load_if_changed(), None);
        assert_eq!(watcher.load_if_changed(), None);
    }

    #[test]
    fn blending_matches_aether() {
        assert_eq!(blend(0xFFFFFF, 0.5, 0x000000), 0x808080);
        assert_eq!(blend(0x102030, 1.0, 0xABCDEF), 0x102030);
        assert_eq!(blend(0x102030, 0.0, 0xABCDEF), 0xABCDEF);
    }
}
