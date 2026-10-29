//! Working directory to build root: walk upward until a directory holds a
//! `build.sbt` or a `project/build.properties`. The nearest wins, so a nested
//! sbt build is its own root.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// A canonical directory that sbt treats as a build. Only
/// [`resolve_build_root`] constructs one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BuildRoot(PathBuf);

impl BuildRoot {
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Where sbt publishes its server connection descriptor.
    #[must_use]
    pub fn active_json(&self) -> PathBuf {
        self.0.join("project").join("target").join("active.json")
    }
}

impl fmt::Display for BuildRoot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.display().fmt(f)
    }
}

#[derive(Debug, Error)]
pub enum RoutingError {
    #[error("no build.sbt or project/build.properties in {cwd} or any parent directory")]
    NoBuildRoot { cwd: PathBuf },
    #[error("cannot resolve {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Resolve the build root that owns `cwd`.
///
/// # Errors
///
/// `NoBuildRoot` when no ancestor is an sbt build; `Io` when `cwd` cannot be
/// canonicalized (it does not exist, or a component is unreadable).
pub fn resolve_build_root(cwd: &Path) -> Result<BuildRoot, RoutingError> {
    let start = cwd.canonicalize().map_err(|source| RoutingError::Io {
        path: cwd.to_path_buf(),
        source,
    })?;
    start
        .ancestors()
        .find(|dir| is_build_root(dir))
        .map(|dir| BuildRoot(dir.to_path_buf()))
        .ok_or(RoutingError::NoBuildRoot { cwd: start })
}

fn is_build_root(dir: &Path) -> bool {
    dir.join("build.sbt").is_file() || dir.join("project").join("build.properties").is_file()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn tree(entries: &[&str]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for entry in entries {
            let path = root.path().join(entry);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            if entry.ends_with('/') {
                fs::create_dir_all(&path).unwrap();
            } else {
                fs::write(&path, "").unwrap();
            }
        }
        root
    }

    #[test]
    fn finds_build_sbt_in_an_ancestor() {
        let root = tree(&["build.sbt", "src/main/scala/"]);
        let found = resolve_build_root(&root.path().join("src/main/scala")).unwrap();
        assert_eq!(found.as_path(), root.path().canonicalize().unwrap());
    }

    #[test]
    fn finds_build_properties_without_build_sbt() {
        let root = tree(&["project/build.properties", "sub/"]);
        let found = resolve_build_root(&root.path().join("sub")).unwrap();
        assert_eq!(found.as_path(), root.path().canonicalize().unwrap());
    }

    #[test]
    fn nearest_build_wins_for_nested_builds() {
        let root = tree(&["build.sbt", "inner/build.sbt", "inner/src/"]);
        let found = resolve_build_root(&root.path().join("inner/src")).unwrap();
        assert_eq!(
            found.as_path(),
            root.path().join("inner").canonicalize().unwrap()
        );
    }

    #[test]
    fn a_directory_named_build_sbt_does_not_count() {
        let root = tree(&["build.sbt/", "work/"]);
        let err = resolve_build_root(&root.path().join("work")).unwrap_err();
        assert!(matches!(err, RoutingError::NoBuildRoot { .. }), "{err}");
    }

    #[test]
    fn missing_cwd_is_an_io_error() {
        let root = tree(&[]);
        let err = resolve_build_root(&root.path().join("nope")).unwrap_err();
        assert!(matches!(err, RoutingError::Io { .. }), "{err}");
    }

    #[test]
    fn active_json_lives_under_project_target() {
        let root = tree(&["build.sbt"]);
        let found = resolve_build_root(root.path()).unwrap();
        assert!(found.active_json().ends_with("project/target/active.json"));
    }
}
