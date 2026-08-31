//! Canonical workspace API snapshot capture.

/// Serial in-memory capture orchestration.
pub mod capture;
/// Cargo workspace discovery and feature routing.
mod discovery;
/// Format 1 ownership marker representation.
mod manifest;
/// Public snapshot value types and profile resolution.
mod model;
/// Safe snapshot tree comparison and persistence.
mod store;

pub use capture::capture;
pub use model::{
    ApiSnapshot, CrateSnapshot, SnapshotFeatures, SnapshotProfile, SnapshotProfileOptions,
    SnapshotRequest,
};
pub use store::{SnapshotChange, SnapshotChangeKind, SnapshotMode, SnapshotReport, SnapshotStore};
