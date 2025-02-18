use std::{
    fmt::{Display, Formatter},
    ops::Deref,
    str::FromStr,
    sync::Arc,
    time::SystemTime,
};

use ahash::HashMap;
use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use digest::OutputSizeUser;
use generic_array::GenericArray;
use md5::Md5;
use sha1::Sha1;
use trait_enum::trait_enum;

use super::{local_dir::LocalDir, Error, FsMetadata, Metadata, OpenFlags};

/// A virtual filesystem backend suitable for exposing over the network using
/// Schlep.
#[async_trait]
pub trait Vfs: Send {
    /// Opens the file at `path` using the `flags` provided.
    async fn open(&self, path: &Utf8Path, flags: OpenFlags) -> Result<Handle, Error>;

    /// Opens the directory at `path`.
    async fn open_dir(&self, path: &Utf8Path) -> Result<Handle, Error>;
    async fn close(&self, handle: Handle) -> Result<(), Error>;
    async fn owns_handle(&self, handle: &Handle) -> bool;

    /// Reads `len` bytes from the file represented by the given `handle`,
    /// starting at `offset` bytes from the start of the file.
    ///
    /// If there are less than `len` bytes in the file starting at `offset`,
    /// returns a shorter buffer containing the remaining bytes. If
    /// attempting to read from a file starting at or past the `EOF`, returns
    /// [`None`].
    ///
    /// This function should use its asynchrony to ensure that it only returns
    /// early in the case of an actual error or reaching the end of the file,
    /// and ensure that otherwise the length of the returned vector is equal to
    /// the requested `len`.
    async fn read(
        &self,
        handle: &Handle,
        offset: usize,
        len: usize,
    ) -> Result<Option<Vec<u8>>, Error>;

    async fn read_dir(&self, handle: &Handle) -> Result<Vec<(Utf8PathBuf, Metadata)>, Error>;
    async fn write(&self, handle: &Handle, offset: usize, data: &[u8]) -> Result<(), Error>;
    async fn stat_fd(&self, handle: &Handle) -> Result<Metadata, Error>;
    async fn sync_fd(&self, handle: &Handle) -> Result<(), Error>;

    async fn rename(&self, from: &Utf8Path, to: &Utf8Path) -> Result<(), Error>;

    async fn stat(&self, path: &Utf8Path) -> Result<Metadata, Error>;
    async fn stat_link(&self, path: &Utf8Path) -> Result<Metadata, Error>;
    async fn statvfs(&self, path: &Utf8Path) -> Result<FsMetadata, Error>;

    async fn hardlink(&self, path: &Utf8Path, target: &Utf8Path) -> Result<(), Error>;

    async fn symlink(&self, path: &Utf8Path, target: &Utf8Path) -> Result<(), Error>;

    async fn md5sum(
        &self,
        path: &Utf8Path,
    ) -> Result<GenericArray<u8, <Md5 as OutputSizeUser>::OutputSize>, Error>;
    async fn sha1sum(
        &self,
        path: &Utf8Path,
    ) -> Result<GenericArray<u8, <Sha1 as OutputSizeUser>::OutputSize>, Error>;

    async fn readlink(&self, path: &Utf8Path) -> Result<Utf8PathBuf, Error>;
    async fn mkdir(&self, path: &Utf8Path) -> Result<(), Error>;
    async fn remove_file(&self, path: &Utf8Path) -> Result<(), Error>;
    async fn remove_dir(&self, path: &Utf8Path) -> Result<(), Error>;

    async fn set_times(
        &self,
        path: &Utf8Path,
        atime: Option<SystemTime>,
        mtime: Option<SystemTime>,
    ) -> Result<(), Error>;

    async fn set_times_fd(
        &self,
        handle: &Handle,
        atime: Option<SystemTime>,
        mtime: Option<SystemTime>,
    ) -> Result<(), Error>;
}

/// A trait representing a file handle within the VFS implementation.
///
/// [`Handle::render_handle`] must be a relation-preserving isomorphism with
/// respect to handle identity and string equality. That is, for two handles `x`
/// and `y`, `x.render_handle() == y.render_handle()` if and only if `x` and `y`
/// point to the same underlying file or directory. Obviously, as a consequence,
/// `x.render_handle() == y.render_handle()` necessarily implies
/// `x.handle_type() == y.handle_type()`.
///
/// Handles must be *globally unique* within the program! In combination with
/// the other requirement, this means that no two instantiations of types
/// implementing [`Vfs`] in the same program may produce handles that render out
/// to the same string. This is because the SFTP protocol references open files
/// and directories purely by their handles, so the sftp needs to be able to
/// use these handles to determine what underlying VFS they belong to.
///
/// It is essential that these raw `vfs_handle`s must be 250 bytes or less, due
/// to limits in the underlying SFTP protocol.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Handle {
    handle_ty: HandleType,
    vfs_handle: String,
}

impl Handle {
    /// Generate a handle for an open file, based on the internal `vfs_handle`.
    pub fn file(vfs_handle: String) -> Self {
        Handle {
            handle_ty: HandleType::File,
            vfs_handle,
        }
    }

    /// Generate a handle for an open directory, based on the internal
    /// `vfs_handle`.
    pub fn dir(vfs_handle: String) -> Self {
        Handle {
            handle_ty: HandleType::Dir,
            vfs_handle,
        }
    }

    /// Reports if this handle represents a file or a directory.
    pub fn handle_type(&self) -> HandleType {
        self.handle_ty
    }

    /// Get a reference to the internal `vfs_handle`.
    pub fn vfs_handle(&self) -> &str {
        self.vfs_handle.as_str()
    }
}

impl FromStr for Handle {
    type Err = HandleParseError;

    fn from_str(handle: &str) -> Result<Self, Self::Err> {
        if handle.starts_with("dir") {
            let handle = handle[4..].to_string();
            Ok(Handle::dir(handle))
        } else if handle.starts_with("file") {
            let handle = handle[5..].to_string();
            Ok(Handle::file(handle))
        } else {
            Err(HandleParseError::InvalidHandle(handle.to_string()))
        }
    }
}

impl Display for Handle {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.handle_ty {
            HandleType::File => write!(f, "file_{}", self.vfs_handle),
            HandleType::Dir => write!(f, "dir_{}", self.vfs_handle),
        }
    }
}

/// Errors that can arise while parsing a [`Handle`] from its [`String`]
/// representation.
#[derive(thiserror::Error, Debug)]
pub enum HandleParseError {
    #[error("invalid handle: {0}")]
    InvalidHandle(String),
}

/// A simple marker that allows code to distinguish between handles associated
/// with files and handles associated with directories.
///
/// An implementation of [Vfs] may or may not use [Handle]s that are internally
/// represented by a type with a shared namespace, but from the perspective of a
/// consumer, file and directory handles are separate.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum HandleType {
    File,
    Dir,
}

/// A `VfsSet` tracks the tree of configured virtual file systems,
#[derive(Clone)]
pub struct VfsSet {
    vfs_map: HashMap<Utf8PathBuf, (usize, Arc<VfsInstance>)>,
}

/// An opaque wrapper for an implementor of [`Vfs`].
#[repr(transparent)]
pub struct VfsInstance {
    inner: VfsInstanceInner,
}

impl VfsInstance {
    #[allow(non_snake_case)]
    pub(super) fn LocalDir(local_dir: LocalDir) -> Self {
        Self {
            inner: VfsInstanceInner::LocalDir(local_dir),
        }
    }
}

impl Deref for VfsInstance {
    type Target = dyn Vfs;

    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

trait_enum! {
    enum VfsInstanceInner: Vfs {
            LocalDir
        }
}

#[derive(Clone)]
/// A match returned by [`VfsSet::resolve_path`].
pub struct PathMatch {
    /// The path, relative to the root of the VFS.
    pub relative_path: Utf8PathBuf,
    /// The matching VFS itself.
    pub vfs: Arc<VfsInstance>,
}

impl VfsSet {
    fn new(vfs_map: HashMap<Utf8PathBuf, (usize, Arc<VfsInstance>)>) -> Self {
        Self { vfs_map }
    }

    pub fn resolve_path(&self, path: &Utf8Path) -> Option<PathMatch> {
        use pathdiff::diff_utf8_paths;

        let mut best_match: Option<(Utf8PathBuf, &Arc<VfsInstance>)> = None;
        let mut longest_match: usize = 0;

        for (prefix, (len, vfs)) in &self.vfs_map {
            if path.starts_with(prefix) && *len > longest_match {
                let relative_path = diff_utf8_paths(path, prefix).unwrap();

                if relative_path == "" {
                    let relative_path = Utf8PathBuf::from(".");

                    return Some(PathMatch {
                        relative_path,
                        vfs: vfs.clone(),
                    });
                }

                best_match = Some((relative_path, vfs));
                longest_match = *len;
            }
        }

        best_match.map(|(path, vfs)| PathMatch {
            relative_path: path,
            vfs: vfs.clone(),
        })
    }

    pub async fn resolve_handle(&self, handle: &Handle) -> Option<Arc<VfsInstance>> {
        for (_, vfs) in self.vfs_map.values() {
            if vfs.owns_handle(handle).await {
                return Some(Arc::clone(vfs));
            }
        }

        None
    }
}

/// A builder for creating an immutable [`VfsSet`].
pub struct VfsSetBuilder {
    vfs_map: HashMap<Utf8PathBuf, (usize, Arc<VfsInstance>)>,
}

impl VfsSetBuilder {
    /// Construct a new [`VfsSetBuilder`].
    pub fn new() -> Self {
        Self {
            vfs_map: HashMap::default(),
        }
    }

    /// Add a new [`LocalDir`] to the VFS set.
    pub fn local_dir(
        mut self,
        vfs_root: Utf8PathBuf,
        local_dir: Utf8PathBuf,
    ) -> Result<Self, Error> {
        let num_components = vfs_root.components().count();

        self.vfs_map.insert(
            vfs_root.clone(),
            (
                num_components,
                Arc::new(VfsInstance::LocalDir(LocalDir::new(vfs_root, local_dir)?)),
            ),
        );

        Ok(self)
    }

    /// Build a [`VfsSet`] that can be provided to a sftp that uses the VFS
    /// interface.
    pub fn build(&self) -> VfsSet {
        VfsSet::new(self.vfs_map.clone())
    }
}

impl Default for VfsSetBuilder {
    fn default() -> Self {
        Self::new()
    }
}
