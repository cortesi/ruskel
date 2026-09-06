//! Integration tests for search-driven container expansion.

mod utils;

#[cfg(test)]
mod tests {
    use libruskel::{
        CrateRequest, Ruskel, SearchDomain, SearchItemKind, SearchOptions, SearchResponse,
    };
    use tempfile::TempDir;

    use super::utils::{create_test_crate, isolated_ruskel};

    /// Temporary crate and cache used by search API regressions.
    struct SearchFixture {
        _workspace: TempDir,
        _cache: TempDir,
        target: String,
        ruskel: Ruskel,
    }

    impl SearchFixture {
        /// Compile a source fixture and isolate its Ruskel cache.
        fn new(source: &str) -> Self {
            let (workspace, target) = create_test_crate(source, false);
            let (cache, ruskel) = isolated_ruskel();
            Self {
                _workspace: workspace,
                _cache: cache,
                target,
                ruskel: ruskel.with_frontmatter(false),
            }
        }

        /// Run one public search request against the fixture.
        fn search(&self, request: &CrateRequest, options: &SearchOptions) -> SearchResponse {
            self.ruskel
                .search(&self.target, request, options)
                .expect("search fixture should compile and search")
        }
    }

    /// Build a name-only search with the requested container policy.
    fn name_search(query: &str, expand_containers: bool) -> SearchOptions {
        SearchOptions::configured(query, SearchDomain::NAMES, false, expand_containers)
    }

    /// Reduce search results to stable kind and path assertions.
    fn summary(response: &SearchResponse) -> Vec<(SearchItemKind, String)> {
        response
            .results
            .iter()
            .map(|result| (result.kind, result.path_string.clone()))
            .collect()
    }

    #[test]
    fn enum_name_search_expands_nested_variant_fields() {
        let fixture = SearchFixture::new(
            r#"
                pub enum Mode {
                    Off,
                    Nested {
                        first: u8,
                        second: u16,
                    },
                    Pair(u32, u64),
                }
            "#,
        );
        let request = CrateRequest::default();

        let expanded = fixture.search(&request, &name_search("Mode", true));
        assert_eq!(
            summary(&expanded),
            vec![(SearchItemKind::Enum, "dummy_crate::Mode".to_string())]
        );
        assert!(expanded.rendered.contains("pub enum Mode"));
        assert!(expanded.rendered.contains("Off"));
        assert!(expanded.rendered.contains("Nested"));
        assert!(expanded.rendered.contains("first: u8"));
        assert!(expanded.rendered.contains("second: u16"));
        assert!(expanded.rendered.contains("Pair(u32, u64)"));

        let direct = fixture.search(&request, &name_search("Mode", false));
        assert_eq!(summary(&direct), summary(&expanded));
        assert!(direct.rendered.contains("pub enum Mode"));
        assert!(!direct.rendered.contains("Off"));
        assert!(!direct.rendered.contains("Nested"));
        assert!(!direct.rendered.contains("first: u8"));
        assert!(!direct.rendered.contains("Pair(u32, u64)"));
    }

    #[test]
    fn union_name_search_filters_private_fields_and_preserves_order() {
        let fixture = SearchFixture::new(
            r#"
                pub union Bits {
                    pub first: u8,
                    hidden: u16,
                    pub last: u32,
                }
            "#,
        );
        let public_request = CrateRequest::default();

        let expanded = fixture.search(&public_request, &name_search("Bits", true));
        assert_eq!(
            summary(&expanded),
            vec![(SearchItemKind::Union, "dummy_crate::Bits".to_string())]
        );
        assert!(expanded.rendered.contains("pub union Bits"));
        assert!(expanded.rendered.contains("pub first: u8"));
        assert!(expanded.rendered.contains("pub last: u32"));
        assert!(!expanded.rendered.contains("hidden"));
        let first = expanded
            .rendered
            .find("pub first: u8")
            .expect("first field should render");
        let last = expanded
            .rendered
            .find("pub last: u32")
            .expect("last field should render");
        assert!(first < last, "union fields should retain source order");

        let direct = fixture.search(&public_request, &name_search("Bits", false));
        assert_eq!(summary(&direct), summary(&expanded));
        assert!(direct.rendered.contains("pub union Bits"));
        assert!(!direct.rendered.contains("first: u8"));
        assert!(!direct.rendered.contains("hidden"));
        assert!(!direct.rendered.contains("last: u32"));

        let private_request = CrateRequest {
            private_items: true,
            ..CrateRequest::default()
        };
        let private = fixture.search(&private_request, &name_search("Bits", true));
        assert_eq!(summary(&private), summary(&expanded));
        assert!(private.rendered.contains("hidden: u16"));
        let hidden = private
            .rendered
            .find("hidden: u16")
            .expect("private field should render when requested");
        let private_first = private
            .rendered
            .find("pub first: u8")
            .expect("first field should render with private items");
        let private_last = private
            .rendered
            .find("pub last: u32")
            .expect("last field should render with private items");
        assert!(
            private_first < hidden && hidden < private_last,
            "private field should retain source order"
        );
    }
}
