use super::{
    card,
    discovery::{DiscoveredPackage, discover},
};
use crate::{
    cache::CacheHandle,
    error::{Result, RuskelError},
    render::{
        Renderer,
        names::{self, NamedCrate},
    },
    rustdoc_build::{self, CrateReadOptions},
    snapshot::{ApiSnapshot, CrateSnapshot, SnapshotProfile, SnapshotRequest},
    target_resolution::{ResolvedSource, ResolvedTarget},
};

/// Shared inputs for one package capture in a workspace pass.
struct CaptureContext<'a> {
    /// Shared capture profile for every selected member.
    profile: &'a SnapshotProfile,
    /// Whether dependency downloads are disabled.
    offline: bool,
    /// Whether to suppress build and fallback details.
    silent: bool,
    /// Build cache shared by the workspace pass.
    cache: &'a CacheHandle,
    /// Public indexes of members captured earlier in dependency order.
    members: &'a [NamedCrate],
    /// Public indexes of the capture toolchain's standard crates.
    standard: &'a [NamedCrate],
}

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
    let mut members = Vec::with_capacity(discovery.packages.len());
    let standard = names::load_standard_indexes(profile.toolchain())?;

    for package in &discovery.packages {
        let local_features = routed
            .by_package
            .get(&package.package_name)
            .cloned()
            .unwrap_or_default();
        let context = CaptureContext {
            profile: &profile,
            offline,
            silent,
            cache,
            members: &members,
            standard: &standard,
        };
        let (captured, named) = capture_package(package, local_features, &context)?;
        crates.push(captured);
        members.push(named);
    }

    let mut snapshot = ApiSnapshot {
        profile,
        crates,
        skipped_packages: discovery.skipped_packages.clone(),
        index: String::new(),
    };
    snapshot.index = card::workspace_index(&discovery, &snapshot);
    Ok(snapshot)
}

/// Build and render one discovered package under the shared profile.
fn capture_package(
    package: &DiscoveredPackage,
    features: Vec<String>,
    context: &CaptureContext<'_>,
) -> Result<(CrateSnapshot, NamedCrate)> {
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
            no_default_features: !context.profile.features().default_features(),
            all_features: context.profile.features().all_features(),
            features,
            private_items: true,
            hidden_items: true,
            silent: context.silent,
            offline: context.offline,
            bin_override: None,
            toolchain: context.profile.toolchain().to_string(),
            target: Some(context.profile.target().to_string()),
            locked: true,
            cache: context.cache.clone(),
        },
    )
    .map_err(|error| RuskelError::SnapshotCapture {
        package: package.package_name.clone(),
        message: error.to_string(),
    })?;
    let rendered = Renderer::snapshot_v1(context.profile.toolchain())
        .render_catalogue(
            &read.crate_data,
            context.members,
            context.standard,
            &package.dependency_aliases,
        )
        .map_err(|error| RuskelError::SnapshotRender {
            package: package.package_name.clone(),
            message: error.to_string(),
        })?;
    if !context.silent {
        eprintln!(
            "{}: {} definition-path fallbacks",
            package.package_name, rendered.definition_fallbacks
        );
    }
    let docs = read
        .crate_data
        .index
        .get(&read.crate_data.root)
        .and_then(|root| root.docs.as_deref())
        .unwrap_or_default();
    let summary = docs
        .split("\n\n")
        .next()
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ");
    let summary = summary
        .split_once(". ")
        .map(|(sentence, _)| format!("{sentence}."))
        .unwrap_or(summary);
    let contents = format!(
        "{}{}",
        card::crate_card(package, &rendered.unnameable),
        rendered.contents
    );
    let contents = format!("{}\n", contents.trim_end_matches('\n'));

    let named = NamedCrate {
        package_name: package.package_name.clone(),
        crate_name: package.crate_name.clone(),
        index: rendered.public_paths,
    };
    Ok((
        CrateSnapshot {
            package: package.package_name.clone(),
            crate_name: package.crate_name.clone(),
            filename: package.filename.clone(),
            contents,
            summary,
            items: rendered.items,
            exposes: rendered.exposes,
            unnameable: rendered.unnameable,
        },
        named,
    ))
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
