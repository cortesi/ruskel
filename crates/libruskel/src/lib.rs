#![warn(missing_docs)]
//! Ruskel generates skeletonized versions of Rust crates.
//!
//! It produces a single-page, syntactically valid Rust code representation of a crate,
//! with all implementations omitted. This provides a clear overview of the crate's structure
//! and public API.
//!
//! Ruskel works by first fetching all dependencies, then using the nightly Rust toolchain
//! to generate JSON documentation data. This data is then parsed and rendered into
//! the skeletonized format. The skeletonized code is then formatted with rustfmt, and optionally
//! has syntax highlighting applied.
//!
//!
//! You must have the nightly Rust toolchain installed to use (but not to install) Ruskel.

/// Helper utilities for querying Cargo metadata and managing crate sources.
mod cargoutils;
/// Utilities for normalising rustdoc structures before rendering.
mod crateutils;
/// Error types exposed by the libruskel crate.
mod error;
/// Frontmatter formatting and configuration helpers.
mod frontmatter;
pub mod highlight;
/// Identifier helpers shared across rendering code.
mod keywords;
/// Rendering logic that turns rustdoc data into skeleton code.
mod render;
/// Public API surface for driving the renderer.
mod ruskel;
/// Search and indexing utilities used by the CLI.
pub mod search;
/// Target parsing helpers for user-provided specifications.
mod target;

pub use ruskel::Ruskel;

pub use crate::{
    error::{Result, RuskelError},
    render::Renderer,
    search::{
        ListItem, SearchDomain, SearchIndex, SearchItemKind, SearchOptions, SearchPathSegment,
        SearchResponse, SearchResult, describe_domains,
    },
};
