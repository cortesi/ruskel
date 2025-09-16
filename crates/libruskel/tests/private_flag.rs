//! Regression tests for respecting the private item rendering flag.

mod utils;

use rustdoc_types::Visibility;

#[cfg(test)]
mod tests {
    use super::{Visibility, utils::inspect_crate};

    #[test]
    fn inspect_respects_private_flag() {
        let source = r#"
            pub mod inner {
                pub fn exposed() {}
                fn hidden() {}
            }

            pub fn top_level() {}
            fn helper() {}
        "#;

        let public_crate = inspect_crate(source, false, false);
        assert!(!public_crate.includes_private);

        let public_root_id = public_crate
            .index
            .get(&public_crate.root)
            .map(|item| item.crate_id)
            .expect("missing root item");

        assert!(
            public_crate
                .index
                .values()
                .filter(|item| item.crate_id == public_root_id)
                .all(|item| matches!(item.visibility, Visibility::Public))
        );

        assert!(!public_crate.index.values().any(|item| {
            item.crate_id == public_root_id && item.name.as_deref() == Some("hidden")
        }));
        assert!(!public_crate.index.values().any(|item| {
            item.crate_id == public_root_id && item.name.as_deref() == Some("helper")
        }));

        let private_crate = inspect_crate(source, true, false);
        assert!(private_crate.includes_private);

        let private_root_id = private_crate
            .index
            .get(&private_crate.root)
            .map(|item| item.crate_id)
            .expect("missing root item");

        assert!(private_crate.index.values().any(|item| {
            item.crate_id == private_root_id && item.name.as_deref() == Some("hidden")
        }));
        assert!(private_crate.index.values().any(|item| {
            item.crate_id == private_root_id && item.name.as_deref() == Some("helper")
        }));
    }
}
