//! Expected output of the aether port: Omarchy's ristretto palette, and the
//! highlight groups the real aether.nvim (branch v3) resolved from it.

use super::omarchy::{face, Face, NONE};

pub(crate) const AETHER_RISTRETTO_COLORS: &str = r##"mode = "dark"

accent = "#f38d70"
selection = "#403e41"
muted = "#72696a"

background = "#2c2525"
dark_background = "#211b1b"
darker_background = "#181414"
lighter_background = "#3d2f2a"

foreground = "#e6d9db"
dark_foreground = "#72696a"
light_foreground = "#c3b7b8"
bright_foreground = "#e6d9db"

red = "#fd6883"
yellow = "#f9cc6c"
orange = "#fb9a77"
green = "#adda78"
cyan = "#85dacc"
blue = "#f38d70"
magenta = "#a8a9eb"
brown = "#7d4d3b"

bright_red = "#ff8297"
bright_yellow = "#fcd675"
bright_green = "#c8e292"
bright_cyan = "#9bf1e1"
bright_blue = "#f8a788"
bright_magenta = "#bebffd"
"##;

#[rustfmt::skip]
pub(crate) const AETHER_RISTRETTO_FIXTURE: &[(&str, Face)] = &[
    ("Normal", face(0xE6D9DB, 0x2C2525, 0)),
    ("StatusLine", face(0xE6D9DB, 0x211B1B, 0)),
    ("StatusLineNC", face(0x72696A, 0x211B1B, 0)),
    ("Visual", face(NONE, 0x403E41, 0)),
    ("Search", face(0xE6D9DB, 0x7C4F43, 1)),
    ("IncSearch", face(0x2C2525, 0xFB9A77, 1)),
    ("WinSeparator", face(0x72696A, NONE, 1)),
    ("NonText", face(0x72696A, NONE, 0)),
    ("Pmenu", face(0xE6D9DB, 0x211B1B, 0)),
    ("@attribute", face(0x85DACC, NONE, 0)),
    ("@comment", face(0x72696A, NONE, 2)),
    ("@comment.documentation", face(0x72696A, NONE, 2)),
    ("@constant", face(0xFCD675, NONE, 0)),
    ("@constant.builtin", face(0x85DACC, NONE, 0)),
    ("@constructor", face(0xF9CC6C, NONE, 0)),
    ("@string.escape", face(0xBEBFFD, NONE, 0)),
    ("@function", face(0xF38D70, NONE, 1)),
    ("@function.builtin", face(0x85DACC, NONE, 0)),
    ("@function.macro", face(0x85DACC, NONE, 0)),
    ("@function.method", face(0xF38D70, NONE, 1)),
    ("@keyword", face(0xBEBFFD, NONE, 2)),
    ("@label", face(0xF9CC6C, NONE, 0)),
    ("@number", face(0xFB9A77, NONE, 0)),
    ("@operator", face(0xE6D9DB, NONE, 0)),
    ("@property", face(0x9BF1E1, NONE, 0)),
    ("@punctuation.delimiter", face(0x897F80, NONE, 0)),
    ("@punctuation.bracket", face(0x897F80, NONE, 0)),
    ("@punctuation.special", face(0xF8A788, NONE, 0)),
    ("@string", face(0xADDA78, NONE, 0)),
    ("@string.special", face(0xFF8297, NONE, 0)),
    ("@tag", face(0xF9CC6C, NONE, 0)),
    ("@markup.italic", face(NONE, NONE, 2)),
    ("@markup.raw", face(0xADDA78, NONE, 0)),
    ("@markup.link.label", face(0xF38D70, NONE, 0)),
    ("@markup.strong", face(NONE, NONE, 1)),
    ("@markup.heading", face(0xF38D70, NONE, 1)),
    ("@markup.link.url", face(0x85DACC, NONE, 4)),
    ("@markup.link", face(0xF38D70, NONE, 4)),
    ("@type", face(0xF9CC6C, NONE, 1)),
    ("@type.builtin", face(0xE6D9DB, NONE, 0)),
    ("@variable", face(0xE6D9DB, NONE, 0)),
    ("@variable.builtin", face(0xE6D9DB, NONE, 1)),
    ("@variable.parameter", face(0x85DACC, NONE, 0)),
];
