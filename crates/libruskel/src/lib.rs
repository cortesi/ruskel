use std::fs;
use std::path::{Path, PathBuf};

use cargo::core::Workspace;
use cargo::ops;
use cargo::util::context::GlobalContext;

use rustdoc_types::Crate;

mod error;
mod filter;
pub use crate::error::{Result, RuskelError};
pub use crate::filter::Filter;

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

#[derive(Debug)]
pub struct Ruskel {
    /// Path to the Cargo.toml file for the target crate.
    pub manifest_path: PathBuf,

    /// Root directory of the workspace containing the target crate.
    pub workspace_root: PathBuf,

    /// Filtering options for output.
    pub filter: Filter,
}

impl Ruskel {
    pub fn new(target: &str) -> Result<Self> {
        let target_path = PathBuf::from(target);
        let manifest_path = Self::find_manifest(&target_path)?;

        if target_path.exists() {
            let workspace_root = Self::find_workspace_root(&manifest_path)?;
            let filter = Filter::from_path(&target_path, &workspace_root)?;
            Ok(Ruskel {
                manifest_path,
                workspace_root,
                filter,
            })
        } else {
            let workspace_root = Self::find_module(target)?;

            let filter = Filter::from_path(&workspace_root, &workspace_root)?;
            let manifest_path = workspace_root.clone().join("Cargo.toml");
            Ok(Ruskel {
                manifest_path,
                workspace_root,
                filter,
            })
        }
    }

    pub fn json(&self) -> Result<Crate> {
        generate_json(&self.manifest_path)
    }

    pub fn pretty_raw_json(&self) -> Result<String> {
        let crate_data = self.filter.filter_crate(&self.json()?);
        serde_json::to_string_pretty(&crate_data).map_err(RuskelError::JsonParseError)
    }

    fn find_workspace_root(manifest_path: &Path) -> Result<PathBuf> {
        let config = GlobalContext::default()?;
        let workspace = Workspace::new(manifest_path, &config)?;
        Ok(workspace.root().to_path_buf())
    }

    fn find_module(module_name: &str) -> Result<PathBuf> {
        let config = GlobalContext::default()?;
        let workspace = Workspace::new(&Path::new("Cargo.toml").canonicalize()?, &config)?;

        for package in workspace.members() {
            if package.name().as_str() == module_name {
                return Ok(package.manifest_path().parent().unwrap().to_path_buf());
            }
        }

        // Fetch all packages
        let options = ops::FetchOptions {
            gctx: &config,
            targets: vec![],
        };
        let (_, ps) = ops::fetch(&workspace, &options)?;

        for i in ps.packages() {
            if i.name().as_str() == module_name {
                return Ok(i.manifest_path().parent().unwrap().to_path_buf());
            }
        }

        Err(RuskelError::ModuleNotFound(module_name.to_string()))
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

    fn create_cargo_ws(dir: &Path) -> std::io::Result<()> {
        let content = "[workspace]\nmembers = [\"member1\", \"member2\"]";
        fs::write(dir.join("Cargo.toml"), content)
    }

    fn create_cargo_child(dir: &Path, name: &str) -> std::io::Result<()> {
        let content = format!("[package]\nname = \"{}\"\nversion = \"0.1.0\"", name);
        fs::write(dir.join("Cargo.toml"), content)
    }

    fn setup_workspace() -> Result<TempDir> {
        let temp_dir = tempdir()?;
        create_cargo_ws(temp_dir.path())?;

        let member1_dir = temp_dir.path().join("member1");
        fs::create_dir_all(member1_dir.join("src"))?;
        create_cargo_child(&member1_dir, "test-package1")?;
        File::create(member1_dir.join("src").join("lib.rs"))?;

        let member2_dir = temp_dir.path().join("member2");
        fs::create_dir_all(member2_dir.join("src"))?;
        create_cargo_child(&member2_dir, "test-package2")?;
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
        create_cargo_child(temp_dir.path(), "test1")?;
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
        create_cargo_child(temp_dir.path(), "test1")?;
        let non_rust_file = temp_dir.path().join("not_rust.txt");
        File::create(&non_rust_file)?;

        assert!(Ruskel::new(non_rust_file.to_str().unwrap()).is_err());

        Ok(())
    }
}
