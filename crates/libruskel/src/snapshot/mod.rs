//! Canonical workspace API snapshot capture.

/// Serial in-memory capture orchestration.
pub mod capture;
/// Cargo workspace discovery and feature routing.
mod discovery;
/// Public snapshot value types and profile resolution.
mod model;

pub use capture::capture;
pub use model::{
    ApiSnapshot, CrateSnapshot, SnapshotFeatures, SnapshotProfile, SnapshotProfileOptions,
    SnapshotRequest,
};
