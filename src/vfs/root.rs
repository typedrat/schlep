use super::error::Error;
use path_absolutize::Absolutize;
use rustix::fs::{AtFlags, Mode, OFlags, ResolveFlags};
use semver::{Version, VersionReq};
use std::ffi::{CStr, OsStr};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::prelude::OsStrExt;
use std::path::{Path, PathBuf};
use std::string::ToString;
use std::sync::LazyLock;

pub struct Root {
    root: PathBuf,
    root_fd: OwnedFd,
    filename_length: usize,
    cwd: PathBuf,
}
pub(super) static EXPECTED_SYSTEM: &'static str = "Linux";
pub(super) static EXPECTED_VERSION: LazyLock<VersionReq> = LazyLock::new(|| VersionReq {
    comparators: vec![semver::Comparator {
        op: semver::Op::GreaterEq,
        major: 5,
        minor: Some(8),
        patch: None,
        pre: semver::Prerelease::EMPTY,
    }],
});

impl Root {
    pub fn new(root: &Path) -> Result<Self, Error> {
        let uname = rustix::system::uname();
        let found_system = uname.sysname().to_string_lossy().to_string();
        let found_version = parse_release(uname.release());

        if EXPECTED_SYSTEM != found_system
            || !found_version
                .as_ref()
                .map_or(false, |ver| EXPECTED_VERSION.matches(ver))
        {
            return Err(Error::UnsupportedSystem {
                found_system,
                found_version,
            });
        }

        let root_fd = rustix::fs::open(root, OFlags::PATH, Mode::empty())?;
        let filename_length = rustix::fs::fstatvfs(&root_fd)?.f_namemax as usize;
        Ok(Self {
            root: root.to_owned(),
            root_fd,
            filename_length,
            cwd: PathBuf::from("/"),
        })
    }

    pub fn absolute_path(&self, path: &Path) -> Result<PathBuf, Error> {
        let abs_path = path.absolutize_from(&self.cwd).map(PathBuf::from)?;

        Ok(abs_path)
    }

    pub fn dir_fd(&self) -> BorrowedFd {
        self.root_fd.as_fd()
    }

    pub fn open_fd(&self, path: &Path, oflags: OFlags, mode: Mode) -> Result<OwnedFd, Error> {
        let fd = rustix::fs::openat2(
            &self.root_fd,
            path,
            oflags,
            mode,
            ResolveFlags::IN_ROOT | ResolveFlags::NO_MAGICLINKS,
        )?;

        Ok(fd)
    }

    pub fn remove(&self, path: &Path) -> Result<(), Error> {
        let abs_path = self.absolute_path(path)?;
        let abs_path = PathBuf::from(".").join(abs_path);

        rustix::fs::unlinkat(&self.root_fd, abs_path, AtFlags::empty())?;

        Ok(())
    }

    pub fn remove_dir(&self, path: &Path) -> Result<(), Error> {
        let abs_path = self.absolute_path(path)?;

        if abs_path != PathBuf::from("/") {
            let abs_path = PathBuf::from(".").join(abs_path);

            rustix::fs::unlinkat(&self.root_fd, abs_path, AtFlags::REMOVEDIR)?;

            Ok(())
        } else {
            Err(Error::InvalidPath(path.to_path_buf()))
        }
    }

    pub fn readlink(&self, path: &Path) -> Result<PathBuf, Error> {
        let abs_path = self.absolute_path(path)?;
        let abs_path = PathBuf::from(".").join(abs_path);

        let link_cstr = rustix::fs::readlinkat(&self.root_fd, abs_path, Vec::new())?;
        let link_path = Path::new(OsStr::from_bytes(link_cstr.to_bytes()));

        if link_path.starts_with(&self.root) {
            let rel_path = link_path.strip_prefix(&self.root).unwrap();

            Ok(self.cwd.as_path().join(rel_path))
        } else {
            Err(Error::InvalidPath(path.to_path_buf()))
        }
    }
}

fn parse_release(kernel_version: &CStr) -> Option<Version> {
    let version_str = kernel_version.to_str().ok()?;
    let base_version = version_str
        .split_once('-')
        .map_or(version_str, |(version, _)| version);

    let mut parts = base_version.split('.');

    let major: u64 = parts.next()?.parse().ok()?;
    let minor: u64 = parts.next()?.parse().ok()?;
    let patch: u64 = parts.next().unwrap_or("0").parse().ok()?;

    Some(Version::new(major, minor, patch))
}
