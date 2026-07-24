//! Integration coverage for dedicated cache selection and build isolation.

#[cfg(test)]
mod tests {
    use std::{
        env,
        ffi::OsString,
        fs,
        path::{Path, PathBuf},
        sync::{Mutex, MutexGuard},
    };

    use libruskel::{CrateRequest, Result, Ruskel};
    use once_cell::sync::Lazy;
    use tempfile::{TempDir, tempdir};

    static ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    struct CacheEnv {
        _lock: MutexGuard<'static, ()>,
        previous: Option<OsString>,
    }

    impl CacheEnv {
        fn set(path: &Path) -> Self {
            let lock = ENV_LOCK.lock().expect("cache environment lock");
            let previous = env::var_os("RUSKEL_CACHE_DIR");
            // SAFETY: this test serializes all cache-environment mutation in this target.
            unsafe { env::set_var("RUSKEL_CACHE_DIR", path) };
            Self {
                _lock: lock,
                previous,
            }
        }
    }

    impl Drop for CacheEnv {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => {
                    // SAFETY: the guard still holds the environment lock.
                    unsafe { env::set_var("RUSKEL_CACHE_DIR", value) };
                }
                None => {
                    // SAFETY: the guard still holds the environment lock.
                    unsafe { env::remove_var("RUSKEL_CACHE_DIR") };
                }
            }
        }
    }

    fn workspace_fixture() -> Result<(TempDir, PathBuf, PathBuf)> {
        let temp = tempdir()?;
        fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nresolver = \"2\"\nmembers = [\"first\", \"second\"]\n",
        )?;
        let first = write_member(temp.path(), "first")?;
        let second = write_member(temp.path(), "second")?;
        Ok((temp, first, second))
    }

    fn write_member(root: &Path, name: &str) -> Result<PathBuf> {
        let member = root.join(name);
        fs::create_dir_all(member.join("src"))?;
        fs::write(
            member.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\ndoctest = false\n"
            ),
        )?;
        fs::write(
            member.join("src/lib.rs"),
            format!("pub fn {name}_value() -> u8 {{ 1 }}\n"),
        )?;
        Ok(member)
    }

    #[test]
    fn explicit_cache_root_overrides_environment_and_none_restores_it() -> Result<()> {
        let temp = tempdir()?;
        let environment_root = temp.path().join("environment");
        let explicit_root = temp.path().join("explicit");
        let _environment = CacheEnv::set(&environment_root);

        let explicit = Ruskel::new()
            .with_cache_dir(Some(explicit_root.clone()))
            .cache_status()?;
        assert_eq!(explicit.root(), fs::canonicalize(explicit_root)?);

        let restored = Ruskel::new()
            .with_cache_dir(Some(temp.path().join("unused")))
            .with_cache_dir(None)
            .cache_status()?;
        assert_eq!(restored.root(), fs::canonicalize(environment_root)?);
        Ok(())
    }

    #[test]
    fn local_builds_are_isolated_and_workspace_members_share_an_entry() -> Result<()> {
        let (workspace, first, second) = workspace_fixture()?;
        let cache_root = workspace.path().join("ruskel-cache");
        let ruskel = Ruskel::new()
            .with_cache_dir(Some(cache_root))
            .with_silent(true);

        ruskel.inspect(
            first.to_str().expect("UTF-8 path"),
            &CrateRequest::default(),
        )?;
        ruskel.inspect(
            second.to_str().expect("UTF-8 path"),
            &CrateRequest::default(),
        )?;

        assert!(!workspace.path().join("target").exists());
        assert!(!first.join("target").exists());
        assert!(!second.join("target").exists());
        let status = ruskel.cache_status()?;
        assert_eq!(status.toolchains().len(), 1);
        assert_eq!(status.toolchains()[0].workspaces().len(), 1);
        let workspace_status = &status.toolchains()[0].workspaces()[0];
        assert_eq!(
            workspace_status.workspace_root(),
            Some(fs::canonicalize(workspace.path())?.as_path())
        );
        assert_eq!(
            workspace_status.packages(),
            &["first 0.1.0", "second 0.1.0"]
        );
        Ok(())
    }

    #[test]
    fn standard_library_query_bypasses_invalid_cache_root() -> Result<()> {
        let temp = tempdir()?;
        let invalid_root = temp.path().join("not-a-directory");
        fs::write(&invalid_root, b"foreign")?;

        let crate_data = Ruskel::new()
            .with_cache_dir(Some(invalid_root))
            .with_silent(true)
            .inspect("std::vec::Vec", &CrateRequest::default())?;

        assert!(!crate_data.index.is_empty());
        Ok(())
    }
}
