//! Syntax highlighting functionality for Rust code.

use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};

use crate::{Result, RuskelError};

/// Applies syntax highlighting to Rust code using the Solarized (dark) theme.
///
/// # Arguments
/// * `code` - The Rust code to highlight
///
/// # Returns
/// A string with ANSI escape codes for terminal color output
pub fn highlight_code(code: &str) -> Result<String> {
    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();

    let syntax = ss
        .find_syntax_by_extension("rs")
        .ok_or_else(|| RuskelError::Highlight("Rust syntax not found".to_string()))?;
    let mut h = HighlightLines::new(syntax, &ts.themes["Solarized (dark)"]);

    let mut output = String::new();
    for line in LinesWithEndings::from(code) {
        let ranges: Vec<(syntect::highlighting::Style, &str)> = h.highlight_line(line, &ss)?;
        let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
        output.push_str(&escaped);
    }

    Ok(output)
}
