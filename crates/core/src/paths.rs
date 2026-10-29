//! The daemon's state directory: `$XDG_CONFIG_HOME/sbt-server`, else
//! `~/.config/sbt-server`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDir(PathBuf);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PathsError {
    #[error(
        "neither XDG_CONFIG_HOME nor HOME is set; cannot locate the sbt-server state directory"
    )]
    NoHome,
}

impl StateDir {
    /// The directory the current environment designates.
    ///
    /// # Errors
    ///
    /// `NoHome` when neither `XDG_CONFIG_HOME` nor `HOME` is set.
    pub fn from_env() -> Result<Self, PathsError> {
        Self::resolve(
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        )
    }

    /// Pure core of [`StateDir::from_env`]: `xdg_config_home` wins when set
    /// and non-empty, otherwise `home/.config`.
    ///
    /// # Errors
    ///
    /// `NoHome` when neither value is usable.
    pub fn resolve(
        xdg_config_home: Option<OsString>,
        home: Option<OsString>,
    ) -> Result<Self, PathsError> {
        let non_empty = |value: Option<OsString>| value.filter(|v| !v.is_empty());
        if let Some(xdg) = non_empty(xdg_config_home) {
            return Ok(Self(PathBuf::from(xdg).join("sbt-server")));
        }
        non_empty(home)
            .map(|home| Self(PathBuf::from(home).join(".config").join("sbt-server")))
            .ok_or(PathsError::NoHome)
    }

    #[must_use]
    pub fn at(path: PathBuf) -> Self {
        Self(path)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Holds the running daemon's pid.
    #[must_use]
    pub fn pid_file(&self) -> PathBuf {
        self.0.join("sbtd.pid")
    }

    /// Holds the address the running daemon actually bound, which matters
    /// when it was started on port 0.
    #[must_use]
    pub fn addr_file(&self) -> PathBuf {
        self.0.join("sbtd.addr")
    }

    /// Where a detached daemon writes its stdout and stderr.
    #[must_use]
    pub fn log_file(&self) -> PathBuf {
        self.0.join("sbtd.log")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_config_home_wins() {
        let dir = StateDir::resolve(Some("/xdg".into()), Some("/home/u".into())).unwrap();
        assert_eq!(dir.path(), Path::new("/xdg/sbt-server"));
    }

    #[test]
    fn empty_xdg_falls_back_to_home() {
        let dir = StateDir::resolve(Some(OsString::new()), Some("/home/u".into())).unwrap();
        assert_eq!(dir.path(), Path::new("/home/u/.config/sbt-server"));
        assert_eq!(
            dir.pid_file(),
            Path::new("/home/u/.config/sbt-server/sbtd.pid")
        );
        assert_eq!(
            dir.log_file(),
            Path::new("/home/u/.config/sbt-server/sbtd.log")
        );
    }

    #[test]
    fn nothing_set_is_an_error() {
        assert_eq!(StateDir::resolve(None, None), Err(PathsError::NoHome));
    }
}
