//! Integration tests covering the listing mode output.
#![allow(clippy::tests_outside_test_module)]

mod utils;

use libruskel::{Ruskel, SearchDomain, SearchOptions};
use pretty_assertions::assert_eq;
use utils::create_test_crate;

#[test]
fn list_respects_visibility_flags() {
    let source = r#"
        pub mod top {
            pub struct Widget;
            struct PrivateWidget;
        }

        struct PrivateType;
        pub struct PublicType;
    "#;

    let (_temp_dir, target) = create_test_crate(source, false);
    let ruskel = Ruskel::new().with_offline(true).with_silent(true);

    let public_items = ruskel
        .list(&target, false, false, Vec::new(), false, None)
        .unwrap();
    let public_paths: Vec<String> = public_items.into_iter().map(|item| item.path).collect();

    assert!(public_paths.contains(&"dummy_crate".to_string()));
    assert!(public_paths.contains(&"dummy_crate::top".to_string()));
    assert!(public_paths.contains(&"dummy_crate::top::Widget".to_string()));
    assert!(public_paths.contains(&"dummy_crate::PublicType".to_string()));
    assert!(
        !public_paths
            .iter()
            .any(|path| path.ends_with("PrivateType"))
    );
    assert!(
        !public_paths
            .iter()
            .any(|path| path.ends_with("PrivateWidget"))
    );

    let items_with_private = ruskel
        .list(&target, false, false, Vec::new(), true, None)
        .unwrap();
    let private_paths: Vec<String> = items_with_private
        .iter()
        .map(|item| item.path.clone())
        .collect();
    assert!(
        private_paths
            .iter()
            .any(|path| path.ends_with("PrivateType"))
    );
    assert!(
        private_paths
            .iter()
            .any(|path| path.ends_with("PrivateWidget"))
    );
}

#[test]
fn list_applies_search_filters() {
    let source = r#"
        pub mod alpha {
            pub struct Widget;
            pub fn helper() {}
        }

        pub struct Gadget;
    "#;

    let (_temp_dir, target) = create_test_crate(source, false);
    let ruskel = Ruskel::new().with_offline(true).with_silent(true);

    let mut options = SearchOptions::new("widget");
    options.domains = SearchDomain::NAMES;
    options.include_private = false;

    let filtered = ruskel
        .list(&target, false, false, Vec::new(), false, Some(&options))
        .unwrap();

    let filtered_pairs: Vec<(String, String)> = filtered
        .into_iter()
        .map(|item| (item.kind.label().to_string(), item.path))
        .collect();

    assert_eq!(
        filtered_pairs,
        vec![(
            "struct".to_string(),
            "dummy_crate::alpha::Widget".to_string()
        )]
    );
}
