//! Identifier and keyword helpers used while rendering skeletons.

/// Rust reserved words that require raw identifier handling.
pub const RESERVED_WORDS: &[&str] = &[
    "abstract",
    "as",
    "async",
    "await",
    "become",
    "box",
    "break",
    "const",
    "continue",
    "crate",
    "do",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "final",
    "fn",
    "for",
    "gen",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "macro",
    "macro_rules",
    "match",
    "mod",
    "move",
    "mut",
    "override",
    "priv",
    "pub",
    "ref",
    "return",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "try",
    "type",
    "typeof",
    "union",
    "unsafe",
    "unsized",
    "use",
    "virtual",
    "where",
    "while",
    "yield",
];

/// Determine whether `ident` is a Rust keyword that needs escaping.
pub fn is_reserved_word(ident: &str) -> bool {
    RESERVED_WORDS.contains(&ident)
}

#[cfg(test)]
mod tests {
    use super::is_reserved_word;

    #[test]
    fn rust_2024_keywords_are_reserved() {
        for keyword in ["async", "await", "dyn", "gen", "union", "macro_rules"] {
            assert!(is_reserved_word(keyword), "{keyword} must be reserved");
        }
        assert!(!is_reserved_word("ordinary"));
    }
}
