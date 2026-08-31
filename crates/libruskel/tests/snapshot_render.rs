//! Public-boundary checks for canonical snapshot policy.

#[cfg(test)]
mod tests {
    use libruskel::{SnapshotFeatures, SnapshotProfileOptions};

    #[test]
    fn public_feature_policy_is_canonical_without_a_toolchain() {
        let features = SnapshotFeatures::new(
            false,
            false,
            vec!["beta/std".into(), "alpha/extra".into(), "beta/std".into()],
        )
        .expect("valid feature policy");

        assert!(!features.default_features());
        assert!(!features.all_features());
        assert_eq!(features.features(), ["alpha/extra", "beta/std"]);
        assert_eq!(
            SnapshotProfileOptions::new()
                .with_features(features.clone())
                .features(),
            Some(&features)
        );
    }

    #[test]
    fn public_feature_policy_rejects_noncanonical_all_features() {
        assert!(SnapshotFeatures::new(false, true, Vec::new()).is_err());
        assert!(SnapshotFeatures::new(true, true, vec!["extra".into()]).is_err());
    }
}
