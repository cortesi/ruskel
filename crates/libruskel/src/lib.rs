//! Ruskel generates skeletonized versions of Rust crates.
//!
//! It produces a single-page, syntactically valid Rust code representation of a crate,
//! with all implementations omitted. This provides a clear overview of the crate's structure
//! and public API.
//!
//! Ruskel works by first fetching all dependencies, then using the nightly Rust toolchain
//! to generate JSON documentation data. This data is then parsed and rendered into
//! the skeletonized format. The skeltonized code is then formatted with rustfmt, and optionally
//! has syntax highlighting applied.
//!
//!
//! You must have the nightly Rust toolchain installed to use (but not to install) Ruskel.

mod cargoutils;
mod crateutils;
mod error;
mod render;
mod ruskel;
mod target;

pub use crate::error::{Result, RuskelError};
pub use crate::render::Renderer;
pub use ruskel::Ruskel;
