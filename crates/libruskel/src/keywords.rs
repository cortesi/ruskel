//! Identifier and keyword helpers used while rendering skeletons.

/// Rust reserved words that require raw identifier handling.
pub const RESERVED_WORDS: &[&str] = &[
    "abstract", "as", "become", "box", "break", "const", "continue", "crate", "do", "else", "enum",
    "extern", "false", "final", "fn", "for", "if", "impl", "in", "let", "loop", "macro", "match",
    "mod", "move", "mut", "override", "priv", "pub", "ref", "return", "self", "Self", "static",
    "struct", "super", "trait", "true", "try", "type", "typeof", "unsafe", "unsized", "use",
    "virtual", "where", "while", "yield",
];

/// Determine whether `ident` is a Rust keyword that needs escaping.
pub fn is_reserved_word(ident: &str) -> bool {
    RESERVED_WORDS.contains(&ident)
}
