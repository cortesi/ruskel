use super::{
    GENERATED_SOURCE_HEADER,
    discovery::{DiscoveredPackage, discover},
};
use crate::{
    cache::CacheHandle,
    error::{Result, RuskelError},
    render::Renderer,
    rustdoc_build::{self, CrateReadOptions},
    snapshot::{ApiSnapshot, CrateSnapshot, SnapshotProfile, SnapshotRequest},
    target_resolution::{ResolvedSource, ResolvedTarget},
};

/// Discover and capture every selected package without destination I/O.
pub fn capture(
    request: &SnapshotRequest,
    offline: bool,
    silent: bool,
    cache: &CacheHandle,
) -> Result<ApiSnapshot> {
    let discovery = discover(request.inputs(), offline)?;
    let routed = discovery.route_features(request.profile().features())?;
    let profile = request.profile().with_features(routed.canonical);
    let mut crates = Vec::with_capacity(discovery.packages.len());

    for package in &discovery.packages {
        let local_features = routed
            .by_package
            .get(&package.package_name)
            .cloned()
            .unwrap_or_default();
        crates.push(capture_package(
            package,
            &profile,
            local_features,
            offline,
            silent,
            cache,
        )?);
    }

    Ok(ApiSnapshot {
        profile,
        crates,
        skipped_packages: discovery.skipped_packages,
    })
}

/// Build and render one discovered package under the shared profile.
fn capture_package(
    package: &DiscoveredPackage,
    profile: &SnapshotProfile,
    features: Vec<String>,
    offline: bool,
    silent: bool,
    cache: &CacheHandle,
) -> Result<CrateSnapshot> {
    let resolved = ResolvedTarget {
        source: ResolvedSource::Package {
            manifest_path: package.manifest_path.clone(),
        },
        filter: String::new(),
        root_target: None,
    };
    let read = rustdoc_build::build(
        &resolved,
        &CrateReadOptions {
            no_default_features: !profile.features().default_features(),
            all_features: profile.features().all_features(),
            features,
            private_items: true,
            hidden_items: true,
            silent,
            offline,
            bin_override: None,
            toolchain: profile.toolchain().to_string(),
            target: Some(profile.target().to_string()),
            locked: true,
            cache: cache.clone(),
        },
    )
    .map_err(|error| RuskelError::SnapshotCapture {
        package: package.package_name.clone(),
        message: error.to_string(),
    })?;
    let contents = Renderer::snapshot_v1(profile.toolchain())
        .with_snapshot_prefix(GENERATED_SOURCE_HEADER)
        .render(&read.crate_data)
        .map_err(|error| RuskelError::SnapshotRender {
            package: package.package_name.clone(),
            message: error.to_string(),
        })?;

    Ok(CrateSnapshot {
        package: package.package_name.clone(),
        crate_name: package.crate_name.clone(),
        filename: package.filename.clone(),
        contents,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_api_does_not_need_destination_state() {
        let _capture: fn(&SnapshotRequest, bool, bool, &CacheHandle) -> Result<ApiSnapshot> =
            capture;
    }
}
