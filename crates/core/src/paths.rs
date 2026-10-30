//! The daemon's state directory: `$XDG_CONFIG_HOME/sbt-server`, else
//! `~/.config/sbt-server`. It is private to its owner: the directory is 0700
//! and every file sbtd and sbtctl write there is 0600, because `sbtd.log`
//! collects every build's raw output.

use std::ffi::OsString;
use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use thiserror::Error;

const PRIVATE_DIR_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDir(PathBuf);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PathsError {
    #[error(
        "neither XDG_CONFIG_HOME nor HOME is set; cannot locate the sbt-server state directory"
    )]
    NoHome,
}

/// The files sbtd and sbtctl keep in the state directory. Each one knows
/// whether a writer replaces it or appends to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateFile {
    /// The running daemon's pid.
    Pid,
    /// The address the running daemon actually bound, which matters when it
    /// was started on port 0.
    Addr,
    /// Where a detached daemon writes its stdout and stderr.
    Log,
    /// The bearer token the running daemon requires on every route but
    /// `/health`; minted at start, removed at exit, so a restart rotates it.
    Token,
}

impl StateFile {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pid => "sbtd.pid",
            Self::Addr => "sbtd.addr",
            Self::Log => "sbtd.log",
            Self::Token => "sbtd.token",
        }
    }

    fn open_options(self) -> OpenOptions {
        let mut options = OpenOptions::new();
        match self {
            Self::Pid | Self::Addr | Self::Token => options.write(true).truncate(true),
            Self::Log => options.append(true),
        };
        options.create(true).mode(PRIVATE_FILE_MODE);
        options
    }
}

#[derive(Debug, Error)]
pub enum StateDirError {
    #[error("cannot create {}: {source}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} exists but is not a directory", path.display())]
    NotADirectory { path: PathBuf },
    #[error("cannot restrict {} to mode {mode:o}: {source}", path.display())]
    Restrict {
        path: PathBuf,
        mode: u32,
        #[source]
        source: io::Error,
    },
    #[error("cannot open {}: {source}", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
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

    #[must_use]
    pub fn file(&self, file: StateFile) -> PathBuf {
        self.0.join(file.name())
    }

    #[must_use]
    pub fn pid_file(&self) -> PathBuf {
        self.file(StateFile::Pid)
    }

    #[must_use]
    pub fn addr_file(&self) -> PathBuf {
        self.file(StateFile::Addr)
    }

    #[must_use]
    pub fn token_file(&self) -> PathBuf {
        self.file(StateFile::Token)
    }

    #[must_use]
    pub fn log_file(&self) -> PathBuf {
        self.file(StateFile::Log)
    }

    /// Creates the directory with mode 0700, or restricts an existing one
    /// to 0700. Missing parents are created with the default mode; only the
    /// state directory itself is private.
    ///
    /// # Errors
    ///
    /// `Create` when it cannot be created, `NotADirectory` when the path is
    /// taken by something else, `Restrict` when its mode cannot be changed
    /// (it belongs to another user, for one).
    pub fn ensure(&self) -> Result<(), StateDirError> {
        let create = |source| StateDirError::Create {
            path: self.0.clone(),
            source,
        };
        if let Some(parent) = self.0.parent() {
            fs::create_dir_all(parent).map_err(create)?;
        }
        match DirBuilder::new().mode(PRIVATE_DIR_MODE).create(&self.0) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(create(error)),
        }
        let metadata = fs::metadata(&self.0).map_err(create)?;
        if !metadata.is_dir() {
            return Err(StateDirError::NotADirectory {
                path: self.0.clone(),
            });
        }
        restrict(&self.0, metadata.permissions().mode(), PRIVATE_DIR_MODE)
    }

    /// Opens `file` for writing (truncating the pid and addr files,
    /// appending to the log), creating it with mode 0600 and restricting an
    /// existing one, such as a log left by an earlier version, to 0600.
    /// Call [`StateDir::ensure`] first.
    ///
    /// # Errors
    ///
    /// `Open` when it cannot be opened, `Restrict` when its mode cannot be
    /// changed.
    pub fn open(&self, file: StateFile) -> Result<File, StateDirError> {
        let path = self.file(file);
        let handle = file
            .open_options()
            .open(&path)
            .map_err(|source| StateDirError::Open {
                path: path.clone(),
                source,
            })?;
        let current_mode = handle
            .metadata()
            .map_err(|source| StateDirError::Open {
                path: path.clone(),
                source,
            })?
            .permissions()
            .mode();
        restrict(&path, current_mode, PRIVATE_FILE_MODE)?;
        Ok(handle)
    }
}

fn restrict(path: &Path, current_mode: u32, mode: u32) -> Result<(), StateDirError> {
    if current_mode & 0o777 == mode {
        return Ok(());
    }
    fs::set_permissions(path, Permissions::from_mode(mode)).map_err(|source| {
        StateDirError::Restrict {
            path: path.to_path_buf(),
            mode,
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

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

    #[test]
    fn ensure_creates_a_private_directory_and_its_parents() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = StateDir::at(tmp.path().join("config/sbt-server"));
        dir.ensure().unwrap();
        assert_eq!(mode(dir.path()), 0o700);
    }

    #[test]
    fn ensure_restricts_an_existing_open_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sbt-server");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, Permissions::from_mode(0o755)).unwrap();
        StateDir::at(path.clone()).ensure().unwrap();
        assert_eq!(mode(&path), 0o700);
    }

    #[test]
    fn ensure_refuses_a_file_in_the_way() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sbt-server");
        fs::write(&path, "").unwrap();
        assert!(matches!(
            StateDir::at(path).ensure(),
            Err(StateDirError::NotADirectory { .. })
        ));
    }

    #[test]
    fn open_creates_private_files_and_restricts_existing_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = StateDir::at(tmp.path().join("sbt-server"));
        dir.ensure().unwrap();
        fs::write(dir.log_file(), "old\n").unwrap();
        fs::set_permissions(dir.log_file(), Permissions::from_mode(0o644)).unwrap();

        dir.open(StateFile::Log)
            .unwrap()
            .write_all(b"new\n")
            .unwrap();
        writeln!(dir.open(StateFile::Pid).unwrap(), "123456").unwrap();
        write!(dir.open(StateFile::Pid).unwrap(), "42").unwrap();

        assert_eq!(fs::read_to_string(dir.log_file()).unwrap(), "old\nnew\n");
        assert_eq!(fs::read_to_string(dir.pid_file()).unwrap(), "42");
        for file in [StateFile::Log, StateFile::Pid] {
            assert_eq!(mode(&dir.file(file)), 0o600, "{file:?}");
        }
    }
}
