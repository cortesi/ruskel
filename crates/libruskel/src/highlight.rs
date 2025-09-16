//! Syntax highlighting functionality for Rust code.

use once_cell::sync::Lazy;
use syntect::{
    easy::HighlightLines,
    highlighting::{Style, Theme, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
    util::{LinesWithEndings, as_24_bit_terminal_escaped},
};

use crate::{Result, RuskelError};

/// Lazily loaded syntect syntax definitions including newline handling.
static SYNTAX_SET: Lazy<SyntaxSet> = Lazy::new(SyntaxSet::load_defaults_newlines);
/// Shared theme catalog for syntax highlighting.
static THEME_SET: Lazy<ThemeSet> = Lazy::new(ThemeSet::load_defaults);
/// Cached lookup for the Rust syntax definition.
static RUST_SYNTAX: Lazy<Option<&'static SyntaxReference>> =
    Lazy::new(|| SYNTAX_SET.find_syntax_by_extension("rs"));
/// Reference to the Solarized (dark) theme used for highlighting output.
static SOLARIZED_THEME: Lazy<&'static Theme> = Lazy::new(|| {
    THEME_SET
        .themes
        .get("Solarized (dark)")
        .expect("Solarized (dark) theme must exist")
});

/// Applies syntax highlighting to Rust code using the Solarized (dark) theme.
///
/// # Arguments
/// * `code` - The Rust code to highlight
///
/// # Returns
/// A string with ANSI escape codes for terminal color output
pub fn highlight_code(code: &str) -> Result<String> {
    let syntax = *RUST_SYNTAX
        .as_ref()
        .ok_or_else(|| RuskelError::Highlight("Rust syntax not found".to_string()))?;
    let mut h = HighlightLines::new(syntax, *SOLARIZED_THEME);

    let mut output = String::new();
    for line in LinesWithEndings::from(code) {
        let ranges: Vec<(Style, &str)> = h.highlight_line(line, &SYNTAX_SET)?;
        let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
        output.push_str(&escaped);
    }

    Ok(output)
}
