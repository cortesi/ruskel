use std::path::{Path, PathBuf};

use rustdoc_types::{Crate, Item};

use crate::error::{Result, RuskelError};

///
///
/// The filtering options for Ruskel output.
///
/// Specifies how crate data should be filtered before presentation.
#[derive(Debug, PartialEq)]
pub enum Filter {
    /// No filtering applied. Includes all crate items.
    None,

    /// Include only items from the specified file.
    ///
    /// Path is relative to the workspace root.
    File(PathBuf),
}

fn item_matches_file(item: &Item, file_path: &Path) -> bool {
    item.span
        .as_ref()
        .map(|span| Path::new(&span.filename))
        .map_or(false, |item_path| item_path == file_path)
}

impl Filter {
    /// Creates a new Filter based on the target specification and workspace information.
    ///
    /// Currently supports file paths, returning `Filter::File` for .rs files
    /// (with paths relative to the workspace root) and `Filter::None` for other targets.
    pub fn new(target: &str, workspace_root: &Path) -> Result<Self> {
        let target_path = PathBuf::from(target);

        if target_path.extension().map_or(false, |ext| ext == "rs") {
            println!(
                "Filtering by file: {} {}",
                workspace_root.display(),
                target_path.display()
            );
            match target_path.strip_prefix(workspace_root) {
                Ok(relative_path) => Ok(Filter::File(relative_path.to_path_buf())),
                Err(_) => Err(RuskelError::InvalidTargetPath(target_path)),
            }
        } else {
            Ok(Filter::None)
        }
    }

    pub fn filter_crate(&self, crate_data: &Crate) -> Crate {
        match self {
            Filter::None => crate_data.clone(),
            Filter::File(file_path) => {
                let mut filtered_crate = crate_data.clone();
                filtered_crate.index = crate_data
                    .index
                    .iter()
                    .filter_map(|(id, item)| {
                        if item_matches_file(item, file_path) {
                            Some((id.clone(), item.clone()))
                        } else {
                            None
                        }
                    })
                    .collect();
                filtered_crate
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rustdoc_types::{Crate, Id, Item, ItemEnum, Span};
    use std::collections::HashMap;

    fn create_mock_crate() -> Crate {
        let mut index = HashMap::new();

        index.insert(
            Id("0".to_string()),
            Item {
                id: Id("0".to_string()),
                crate_id: 0,
                name: Some("item1".to_string()),
                span: Some(Span {
                    filename: PathBuf::from("src/lib.rs"),
                    begin: (1, 0),
                    end: (1, 10),
                }),
                visibility: rustdoc_types::Visibility::Public,
                docs: None,
                links: HashMap::new(),
                attrs: vec![],
                deprecation: None,
                inner: ItemEnum::Function(rustdoc_types::Function {
                    decl: rustdoc_types::FnDecl {
                        inputs: vec![],
                        output: None,
                        c_variadic: false,
                    },
                    generics: rustdoc_types::Generics {
                        params: vec![],
                        where_predicates: vec![],
                    },
                    header: rustdoc_types::Header {
                        const_: false,
                        unsafe_: false,
                        async_: false,
                        abi: rustdoc_types::Abi::Rust,
                    },
                    has_body: true,
                }),
            },
        );

        index.insert(
            Id("1".to_string()),
            Item {
                id: Id("1".to_string()),
                crate_id: 0,
                name: Some("item2".to_string()),
                span: Some(Span {
                    filename: PathBuf::from("src/other.rs"),
                    begin: (1, 0),
                    end: (1, 10),
                }),
                visibility: rustdoc_types::Visibility::Public,
                docs: None,
                links: HashMap::new(),
                attrs: vec![],
                deprecation: None,
                inner: ItemEnum::Function(rustdoc_types::Function {
                    decl: rustdoc_types::FnDecl {
                        inputs: vec![],
                        output: None,
                        c_variadic: false,
                    },
                    generics: rustdoc_types::Generics {
                        params: vec![],
                        where_predicates: vec![],
                    },
                    header: rustdoc_types::Header {
                        const_: false,
                        unsafe_: false,
                        async_: false,
                        abi: rustdoc_types::Abi::Rust,
                    },
                    has_body: true,
                }),
            },
        );

        Crate {
            root: Id("0".to_string()),
            crate_version: None,
            includes_private: false,
            index,
            paths: HashMap::new(),
            external_crates: HashMap::new(),
            format_version: 0,
        }
    }

    #[test]
    fn test_filter_crate_none() {
        let crate_data = create_mock_crate();
        let filter = Filter::None;
        let filtered_crate = filter.filter_crate(&crate_data);
        assert_eq!(filtered_crate.index.len(), 2);
    }

    #[test]
    fn test_filter_crate_file() {
        let crate_data = create_mock_crate();
        let filter = Filter::File(PathBuf::from("src/lib.rs"));
        let filtered_crate = filter.filter_crate(&crate_data);
        assert_eq!(filtered_crate.index.len(), 1);
        assert!(filtered_crate.index.contains_key(&Id("0".to_string())));
        assert!(!filtered_crate.index.contains_key(&Id("1".to_string())));
    }

    #[test]
    fn test_item_matches_file() {
        let item = Item {
            id: Id("0".to_string()),
            crate_id: 0,
            name: Some("item1".to_string()),
            span: Some(Span {
                filename: PathBuf::from("src/lib.rs"),
                begin: (1, 0),
                end: (1, 10),
            }),
            visibility: rustdoc_types::Visibility::Public,
            docs: None,
            links: HashMap::new(),
            attrs: vec![],
            deprecation: None,
            inner: ItemEnum::Function(rustdoc_types::Function {
                decl: rustdoc_types::FnDecl {
                    inputs: vec![],
                    output: None,
                    c_variadic: false,
                },
                generics: rustdoc_types::Generics {
                    params: vec![],
                    where_predicates: vec![],
                },
                header: rustdoc_types::Header {
                    const_: false,
                    unsafe_: false,
                    async_: false,
                    abi: rustdoc_types::Abi::Rust,
                },
                has_body: true,
            }),
        };

        assert!(item_matches_file(&item, Path::new("src/lib.rs")));
        assert!(!item_matches_file(&item, Path::new("src/other.rs")));
    }

    #[test]
    fn test_filter_new_file() {
        let workspace_root = Path::new("/workspace");
        let target = "/workspace/src/lib.rs";
        let filter = Filter::new(target, workspace_root);
        assert!(matches!(filter, Ok(Filter::File(_))));
        if let Ok(Filter::File(path)) = filter {
            assert_eq!(path, PathBuf::from("src/lib.rs"));
        }
    }

    #[test]
    fn test_filter_new_non_file() {
        let workspace_root = Path::new("/workspace");
        let target = "/workspace";
        let filter = Filter::new(target, workspace_root);
        assert!(matches!(filter, Ok(Filter::None)));
    }
}
