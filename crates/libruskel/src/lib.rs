use std::fs;
use std::path::{Path, PathBuf};

use rustdoc_types::{Crate, Item};

mod error;
pub use crate::error::{Result, RuskelError};

fn generate_json<P: AsRef<Path>>(manifest_path: P) -> Result<Crate> {
    println!("Generating JSON for {}", manifest_path.as_ref().display());
    let json_path = rustdoc_json::Builder::default()
        .toolchain("nightly")
        .manifest_path(manifest_path.as_ref())
        .document_private_items(true)
        .build()
        .map_err(|e| RuskelError::RustdocJsonError(e.to_string()))?;
    let json_content = fs::read_to_string(&json_path)?;
    let crate_data: Crate = serde_json::from_str(&json_content)?;
    Ok(crate_data)
}

fn filter_crate(crate_data: &Crate, file_path: &Path) -> Crate {
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

#[derive(Debug, PartialEq)]
pub enum Filter {
    None,
    File(PathBuf),
}

#[derive(Debug)]
pub struct Ruskel {
    pub manifest_path: PathBuf,
    pub workspace_root: PathBuf,
    pub filter: Filter,
}

impl Ruskel {
    pub fn new(target: &str) -> Result<Self> {
        let target_path = PathBuf::from(target).canonicalize()?;
        let manifest_path = Self::find_manifest(&target_path)?;
        let workspace_root = Self::find_workspace_root(&manifest_path)?;

        let filter =
            if target_path.is_file() && target_path.extension().map_or(false, |ext| ext == "rs") {
                match target_path.strip_prefix(&workspace_root) {
                    Ok(relative_path) => Filter::File(relative_path.to_path_buf()),
                    Err(_) => return Err(RuskelError::InvalidTargetPath(target_path)),
                }
            } else {
                Filter::None
            };

        Ok(Ruskel {
            manifest_path,
            workspace_root,
            filter,
        })
    }

    pub fn json(&self) -> Result<Crate> {
        generate_json(&self.manifest_path)
    }

    pub fn pretty_raw_json(&self) -> Result<String> {
        let crate_data = self.json()?;
        pretty_print_json(&crate_data, &self.filter)
    }

    fn find_workspace_root(manifest_path: &Path) -> Result<PathBuf> {
        let mut current_dir = manifest_path
            .parent()
            .unwrap_or(Path::new("/"))
            .to_path_buf();
        loop {
            let workspace_manifest = current_dir.join("Cargo.toml");
            if workspace_manifest.exists() {
                let content = fs::read_to_string(&workspace_manifest)?;
                if content.contains("[workspace]") {
                    return Ok(current_dir);
                }
            }
            if !current_dir.pop() {
                // If we've reached the root directory, assume the original manifest is the workspace root
                return Ok(manifest_path
                    .parent()
                    .unwrap_or(Path::new("/"))
                    .to_path_buf());
            }
        }
    }

    fn find_manifest(target_path: &Path) -> Result<PathBuf> {
        let mut path = if target_path.is_file() {
            target_path.parent().unwrap_or(Path::new("/")).to_path_buf()
        } else {
            target_path.to_path_buf()
        };

        loop {
            let manifest_path = path.join("Cargo.toml");
            if manifest_path.exists() {
                return Ok(manifest_path);
            }
            if !path.pop() {
                break;
            }
        }
        Err(RuskelError::ManifestNotFound)
    }
}

pub fn pretty_print_json(crate_data: &Crate, filter: &Filter) -> Result<String> {
    match filter {
        Filter::None => {
            serde_json::to_string_pretty(&crate_data).map_err(RuskelError::JsonParseError)
        }
        Filter::File(file_path) => {
            let filtered_crate = filter_crate(crate_data, file_path);
            serde_json::to_string_pretty(&filtered_crate).map_err(RuskelError::JsonParseError)
        }
    }
}

fn item_matches_file(item: &Item, file_path: &Path) -> bool {
    item.span
        .as_ref()
        .map(|span| Path::new(&span.filename))
        .map_or(false, |item_path| item_path == file_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::{tempdir, TempDir};

    macro_rules! assert_path_eq {
        ($left:expr, $right:expr) => {
            assert_eq!(
                $left.canonicalize().unwrap(),
                $right.canonicalize().unwrap()
            )
        };
    }

    fn create_cargo_toml(path: &Path, is_workspace: bool) -> std::io::Result<()> {
        let content = if is_workspace {
            "[workspace]\nmembers = [\"member1\", \"member2\"]"
        } else {
            "[package]\nname = \"test-package\"\nversion = \"0.1.0\""
        };
        fs::write(path, content)
    }

    fn setup_workspace() -> Result<TempDir> {
        let temp_dir = tempdir()?;
        create_cargo_toml(&temp_dir.path().join("Cargo.toml"), true)?;

        let member1_dir = temp_dir.path().join("member1");
        fs::create_dir_all(member1_dir.join("src"))?;
        create_cargo_toml(&member1_dir.join("Cargo.toml"), false)?;
        File::create(member1_dir.join("src").join("lib.rs"))?;

        let member2_dir = temp_dir.path().join("member2");
        fs::create_dir_all(member2_dir.join("src"))?;
        create_cargo_toml(&member2_dir.join("Cargo.toml"), false)?;
        File::create(member2_dir.join("src").join("main.rs"))?;

        Ok(temp_dir)
    }

    #[test]
    fn test_parse_rust_file_in_workspace() -> Result<()> {
        let temp_dir = setup_workspace()?;
        let lib_rs_path = temp_dir.path().join("member1").join("src").join("lib.rs");

        // Ensure the file exists
        assert!(lib_rs_path.exists(), "lib.rs file does not exist");

        let target = Ruskel::new(lib_rs_path.to_str().unwrap())?;
        assert_path_eq!(
            target.manifest_path,
            temp_dir.path().join("member1").join("Cargo.toml")
        );
        assert_path_eq!(target.workspace_root, temp_dir.path());
        assert_eq!(
            target.filter,
            Filter::File(PathBuf::from("member1/src/lib.rs"))
        );

        Ok(())
    }

    #[test]
    fn test_parse_nonexistent_path() {
        let result = Ruskel::new("/path/does/not/exist");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_standalone_crate() -> Result<()> {
        let temp_dir = tempdir()?;
        create_cargo_toml(&temp_dir.path().join("Cargo.toml"), false)?;
        let src_dir = temp_dir.path().join("src");
        fs::create_dir(&src_dir)?;
        File::create(src_dir.join("lib.rs"))?;

        let target = Ruskel::new(temp_dir.path().to_str().unwrap())?;
        assert_path_eq!(target.manifest_path, temp_dir.path().join("Cargo.toml"));
        assert_path_eq!(target.workspace_root, temp_dir.path());
        assert_eq!(target.filter, Filter::None);

        Ok(())
    }

    #[test]
    fn test_parse_workspace_root() -> Result<()> {
        let temp_dir = setup_workspace()?;

        let target = Ruskel::new(temp_dir.path().to_str().unwrap())?;
        assert_path_eq!(target.manifest_path, temp_dir.path().join("Cargo.toml"));
        assert_path_eq!(target.workspace_root, temp_dir.path());
        assert_eq!(target.filter, Filter::None);

        Ok(())
    }

    #[test]
    fn test_parse_workspace_member() -> Result<()> {
        let temp_dir = setup_workspace()?;
        let member1_dir = temp_dir.path().join("member1");

        let target = Ruskel::new(member1_dir.to_str().unwrap())?;
        assert_path_eq!(target.manifest_path, member1_dir.join("Cargo.toml"));
        assert_path_eq!(target.workspace_root, temp_dir.path());
        assert_eq!(target.filter, Filter::None);

        Ok(())
    }

    #[test]
    fn test_parse_non_rust_file() -> Result<()> {
        let temp_dir = tempdir()?;
        create_cargo_toml(&temp_dir.path().join("Cargo.toml"), false)?;
        let non_rust_file = temp_dir.path().join("not_rust.txt");
        File::create(&non_rust_file)?;

        let target = Ruskel::new(non_rust_file.to_str().unwrap())?;
        assert_path_eq!(target.manifest_path, temp_dir.path().join("Cargo.toml"));
        assert_path_eq!(target.workspace_root, temp_dir.path());
        assert_eq!(target.filter, Filter::None);

        Ok(())
    }
}
