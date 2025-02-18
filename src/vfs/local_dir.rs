use std::{
    io,
    io::{SeekFrom, Write},
    path::PathBuf,
    sync::Arc,
    time::SystemTime,
};

use async_trait::async_trait;
use base64ct::{Base64, Encoding};
use camino::{Utf8Path, Utf8PathBuf};
use cap_fs_ext::DirExtUtf8;
use cap_std::{
    ambient_authority,
    fs_utf8::{Dir, File},
};
use digest::OutputSizeUser;
use generic_array::GenericArray;
use md5::Md5;
use rand::Rng;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use whirlwind::ShardMap;

use super::{options::*, Error, Handle, HandleType, Vfs};
use crate::vfs::error::IntoIoError;

pub struct LocalDir {
    vfs_path: Utf8PathBuf,
    root_path: Utf8PathBuf,
    root_dir: Arc<Dir>,
    open_files: ShardMap<String, File, ahash::RandomState>,
    open_dirs: ShardMap<String, Dir, ahash::RandomState>,
}

impl LocalDir {
    pub fn new(vfs_path: Utf8PathBuf, root_path: Utf8PathBuf) -> Result<Self, Error> {
        let root_dir = Arc::new(
            Dir::open_ambient_dir(root_path.as_path(), ambient_authority())
                .into_io_error("failed to open LocalDir root")?,
        );

        Ok(Self {
            vfs_path,
            root_path,
            root_dir,
            open_files: ShardMap::with_hasher(Default::default()),
            open_dirs: ShardMap::with_hasher(Default::default()),
        })
    }

    async fn get_file(&self, handle: &Handle) -> Result<tokio::fs::File, Error> {
        let vfs_handle = String::from(handle.vfs_handle());

        let file = match self.open_files.get(&vfs_handle).await {
            Some(file_match) => {
                let file = file_match
                    .value()
                    .try_clone()
                    .into_io_error("failed to get file handle")?;
                Ok(tokio::fs::File::from_std(file.into_std()))
            }
            None => Err(Error::FileNotFound),
        };

        file
    }

    async fn get_dir(&self, handle: &Handle) -> Result<Dir, Error> {
        let vfs_handle = String::from(handle.vfs_handle());
        let dir_match = self.open_dirs.get(&vfs_handle).await;
        match dir_match {
            Some(dir_match) => Ok(dir_match
                .value()
                .try_clone()
                .into_io_error("failed to get directory handle")?),
            None => Err(Error::FileNotFound),
        }
    }

    async fn hash<Hash: Digest + Write>(
        &self,
        path: &Utf8Path,
    ) -> Result<GenericArray<u8, <Hash as OutputSizeUser>::OutputSize>, Error> {
        let root_dir = self
            .root_dir
            .try_clone()
            .into_io_error("failed to get root directory handle")?;
        let path = path.to_owned();

        let hash = tokio::task::spawn_blocking(move || {
            let mut file = root_dir.open(path).into_io_error("failed opening file")?;
            let mut hasher = Hash::new();
            io::copy(&mut file, &mut hasher).into_io_error("failed to hash file")?;
            Ok(hasher.finalize())
        })
        .await
        .unwrap_or_else(|e| {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }

            panic!("task failed: {e}");
        })?;

        Ok(hash)
    }
}

#[async_trait]
impl Vfs for LocalDir {
    async fn open(&self, path: &Utf8Path, flags: OpenFlags) -> Result<Handle, Error> {
        let root_dir = self.root_dir.clone();
        let path_buf = Utf8PathBuf::from(path);

        let file = tokio::task::spawn_blocking(move || {
            root_dir
                .open_with(&path_buf, &OpenFlags::into(flags))
                .into_io_error(format!("couldn't open file {path_buf}"))
        })
        .await
        .unwrap_or_else(|e| {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }

            panic!("task failed: {e}");
        })?;

        let mut hasher = Sha256::new();
        let mut salt = [0u8; 32];
        rand::rng().fill(&mut salt);

        hasher.update(self.vfs_path.as_str());
        hasher.update(path.as_str());
        hasher.update(salt);

        let vfs_handle = Base64::encode_string(hasher.finalize().as_slice());

        self.open_files.insert(vfs_handle.clone(), file).await;

        Ok(Handle::file(vfs_handle))
    }

    async fn open_dir(&self, path: &Utf8Path) -> Result<Handle, Error> {
        let root_dir = self.root_dir.clone();
        let path_buf = Utf8PathBuf::from(path);

        let dir = tokio::task::spawn_blocking(move || {
            root_dir
                .open_dir(&path_buf)
                .into_io_error("couldn't open directory {path_buf}")
        })
        .await
        .unwrap_or_else(|e| {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }

            panic!("task failed: {e}");
        })?;

        let mut hasher = Sha256::new();
        let mut salt = [0u8; 32];
        rand::rng().fill(&mut salt);

        hasher.update(self.vfs_path.as_str());
        hasher.update(path.as_str());
        hasher.update(salt);

        let vfs_handle = Base64::encode_string(hasher.finalize().as_slice());

        self.open_dirs.insert(vfs_handle.clone(), dir).await;

        Ok(Handle::dir(vfs_handle))
    }

    async fn close(&self, handle: Handle) -> Result<(), Error> {
        match handle.handle_type() {
            HandleType::File => {
                self.open_files
                    .remove(&String::from(handle.vfs_handle()))
                    .await;
            }
            HandleType::Dir => {
                self.open_dirs
                    .remove(&String::from(handle.vfs_handle()))
                    .await;
            }
        }

        Ok(())
    }

    async fn owns_handle(&self, handle: &Handle) -> bool {
        match handle.handle_type() {
            HandleType::File => self
                .open_files
                .get(&String::from(handle.vfs_handle()))
                .await
                .is_some(),
            HandleType::Dir => self
                .open_dirs
                .get(&String::from(handle.vfs_handle()))
                .await
                .is_some(),
        }
    }

    async fn read(
        &self,
        handle: &Handle,
        offset: usize,
        len: usize,
    ) -> Result<Option<Vec<u8>>, Error> {
        if handle.handle_type() == HandleType::File {
            let mut buf: Vec<u8> = Vec::with_capacity(len);
            let mut file = self.get_file(handle).await?;

            file.seek(SeekFrom::Start(offset as u64))
                .await
                .into_io_error("failed to seek file")?;

            let bytes_read = file
                .take(len as u64)
                .read_to_end(&mut buf)
                .await
                .into_io_error("failed to read file")?;

            if bytes_read == 0 && len != 0 {
                Ok(None)
            } else {
                Ok(Some(buf))
            }
        } else {
            Err(Error::NotAFile)
        }
    }

    async fn read_dir(&self, handle: &Handle) -> Result<Vec<(Utf8PathBuf, Metadata)>, Error> {
        if handle.handle_type() == HandleType::Dir {
            let dir = self.get_dir(handle).await?;

            let entries = tokio::task::spawn_blocking(move || {
                let mut files = Vec::new();

                for entry in dir
                    .entries()
                    .into_io_error("couldn't get directory entries")?
                {
                    let entry = entry.into_io_error("couldn't get directory entry")?;

                    let file_name = entry.file_name().into_io_error("couldn't get file name")?;
                    let metadata = dir
                        .metadata(&file_name)
                        .into_io_error("couldn't get file metadata")?;

                    files.push((Utf8PathBuf::from(file_name), Metadata::from(metadata)))
                }

                Ok(files)
            })
            .await
            .unwrap_or_else(|e| {
                if e.is_panic() {
                    std::panic::resume_unwind(e.into_panic());
                }

                panic!("task failed: {e}");
            })?;

            Ok(entries)
        } else {
            Err(Error::NotADirectory)
        }
    }

    async fn write(&self, handle: &Handle, offset: usize, data: &[u8]) -> Result<(), Error> {
        if handle.handle_type() == HandleType::File {
            let mut file = self.get_file(handle).await?;

            file.seek(SeekFrom::Start(offset as u64))
                .await
                .into_io_error("failed to seek file")?;

            file.write_all(data)
                .await
                .into_io_error("failed to write file")?;

            Ok(())
        } else {
            Err(Error::NotAFile)
        }
    }

    async fn stat_fd(&self, handle: &Handle) -> Result<Metadata, Error> {
        if handle.handle_type() == HandleType::File {
            let file = self.get_file(handle).await?;

            let metadata = file
                .metadata()
                .await
                .into_io_error("failed to get file metadata")?;

            Ok(Metadata::from(metadata))
        } else {
            let dir = self.get_dir(handle).await?;

            let metadata = tokio::task::spawn_blocking(move || {
                dir.dir_metadata()
                    .into_io_error("failed to get directory metadata")
            })
            .await
            .unwrap_or_else(|e| {
                if e.is_panic() {
                    std::panic::resume_unwind(e.into_panic());
                }

                panic!("task failed: {e}");
            })?;

            Ok(Metadata::from(metadata))
        }
    }

    async fn sync_fd(&self, handle: &Handle) -> Result<(), Error> {
        if handle.handle_type() == HandleType::File {
            let file = self.get_file(handle).await?;

            file.sync_all().await.into_io_error("failed to sync file")?;

            Ok(())
        } else {
            Err(Error::NotAFile)
        }
    }

    async fn rename(&self, from: &Utf8Path, to: &Utf8Path) -> Result<(), Error> {
        let root_dir = self
            .root_dir
            .try_clone()
            .into_io_error("failed to get root directory handle")?;
        let from = from.to_owned();
        let to = to.to_owned();

        tokio::task::spawn_blocking(move || {
            root_dir
                .rename(from, &root_dir, to)
                .into_io_error("failed to rename file")
        })
        .await
        .unwrap_or_else(|e| {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }

            panic!("task failed: {e}");
        })?;

        Ok(())
    }

    async fn stat(&self, path: &Utf8Path) -> Result<Metadata, Error> {
        let root_dir = self
            .root_dir
            .try_clone()
            .into_io_error("failed to get root directory handle")?;
        let path = path.to_owned();

        let metadata = tokio::task::spawn_blocking(move || {
            root_dir
                .metadata(path)
                .into_io_error("failed to get symlink metadata")
        })
        .await
        .unwrap_or_else(|e| {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }

            panic!("task failed: {e}");
        })?;

        Ok(Metadata::from(metadata))
    }

    async fn stat_link(&self, path: &Utf8Path) -> Result<Metadata, Error> {
        let root_dir = self
            .root_dir
            .try_clone()
            .into_io_error("failed to get root directory handle")?;
        let path = path.to_owned();

        let metadata = tokio::task::spawn_blocking(move || {
            root_dir
                .symlink_metadata(path)
                .into_io_error("failed to get symlink metadata")
        })
        .await
        .unwrap_or_else(|e| {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }

            panic!("task failed: {e}");
        })?;

        Ok(Metadata::from(metadata))
    }

    async fn statvfs(&self, path: &Utf8Path) -> Result<FsMetadata, Error> {
        let root_dir = self
            .root_dir
            .try_clone()
            .into_io_error("failed to get root directory handle")?;
        let path = path.to_owned();

        let fs_metadata = tokio::task::spawn_blocking(move || {
            let file = root_dir.open(&path).into_io_error("failed to open file")?;
            let fs_metadata = rustix::fs::fstatvfs(&file).map_err(|err| {
                io::Error::from(err).into_io_error("failed to get filesystem metadata")
            })?;
            Ok(fs_metadata)
        })
        .await
        .unwrap_or_else(|e| {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }

            panic!("task failed: {e}");
        })?;

        Ok(fs_metadata.into())
    }

    async fn hardlink(&self, source: &Utf8Path, target: &Utf8Path) -> Result<(), Error> {
        let root_dir = self
            .root_dir
            .try_clone()
            .into_io_error("failed to get root directory handle")?;
        let source = source.to_owned();
        let target = target.to_owned();

        tokio::task::spawn_blocking(move || {
            root_dir
                .hard_link(source, &root_dir, target)
                .into_io_error("failed to create hardlink")
        })
        .await
        .unwrap_or_else(|e| {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }

            panic!("task failed: {e}");
        })?;

        Ok(())
    }

    async fn symlink(&self, path: &Utf8Path, target: &Utf8Path) -> Result<(), Error> {
        let root_dir = self
            .root_dir
            .try_clone()
            .into_io_error("failed to get root directory handle")?;
        let path = path.to_owned();
        let relative_target = pathdiff::diff_utf8_paths(&target, &path)
            .ok_or_else(|| Error::InvalidPath(PathBuf::from(target)))?;

        tokio::task::spawn_blocking(move || {
            root_dir
                .symlink(path, relative_target)
                .into_io_error("failed to remove file")
        })
        .await
        .unwrap_or_else(|e| {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }

            panic!("task failed: {e}");
        })?;

        Ok(())
    }

    async fn md5sum(
        &self,
        path: &Utf8Path,
    ) -> Result<GenericArray<u8, <Md5 as OutputSizeUser>::OutputSize>, Error> {
        self.hash::<Md5>(path).await
    }

    async fn sha1sum(
        &self,
        path: &Utf8Path,
    ) -> Result<GenericArray<u8, <Sha1 as OutputSizeUser>::OutputSize>, Error> {
        self.hash::<Sha1>(path).await
    }

    async fn readlink(&self, path: &Utf8Path) -> Result<Utf8PathBuf, Error> {
        let root_dir = self
            .root_dir
            .try_clone()
            .into_io_error("failed to get root directory handle")?;
        let root_path = self.root_path.clone();
        let path = path.to_owned();

        let link_contents = tokio::task::spawn_blocking(move || {
            let link_contents = root_dir
                .read_link_contents(path)
                .into_io_error("failed to read symlink")?;

            if link_contents.is_absolute() {
                if link_contents.starts_with(&root_path) {
                    let relative_path =
                        pathdiff::diff_utf8_paths(&link_contents, &root_path).unwrap();

                    Ok(relative_path)
                } else {
                    Err(Error::WouldEscape)
                }
            } else {
                Ok(link_contents)
            }
        })
        .await
        .unwrap_or_else(|e| {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }

            panic!("task failed: {e}");
        })?;

        Ok(link_contents)
    }

    async fn mkdir(&self, path: &Utf8Path) -> Result<(), Error> {
        let root_dir = self
            .root_dir
            .try_clone()
            .into_io_error("failed to get root directory handle")?;
        let path = path.to_owned();

        tokio::task::spawn_blocking(move || {
            root_dir
                .create_dir(path)
                .into_io_error("failed to create directory")
        })
        .await
        .unwrap_or_else(|e| {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }

            panic!("task failed: {e}");
        })?;

        Ok(())
    }

    async fn remove_file(&self, path: &Utf8Path) -> Result<(), Error> {
        let root_dir = self
            .root_dir
            .try_clone()
            .into_io_error("failed to get root directory handle")?;
        let path = path.to_owned();

        tokio::task::spawn_blocking(move || {
            root_dir
                .remove_file(path)
                .into_io_error("failed to remove file")
        })
        .await
        .unwrap_or_else(|e| {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }

            panic!("task failed: {e}");
        })?;

        Ok(())
    }

    async fn remove_dir(&self, path: &Utf8Path) -> Result<(), Error> {
        let root_dir = self
            .root_dir
            .try_clone()
            .into_io_error("failed to get root directory handle")?;
        let path = path.to_owned();

        tokio::task::spawn_blocking(move || {
            root_dir
                .remove_dir(path)
                .into_io_error("failed to remove directory")
        })
        .await
        .unwrap_or_else(|e| {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }

            panic!("task failed: {e}");
        })?;

        Ok(())
    }

    async fn set_times(
        &self,
        path: &Utf8Path,
        atime: Option<SystemTime>,
        mtime: Option<SystemTime>,
    ) -> Result<(), Error> {
        use cap_primitives::fs::SystemTimeSpec;

        fn convert_system_time(time: SystemTime) -> SystemTimeSpec {
            SystemTimeSpec::Absolute(cap_primitives::time::SystemTime::from_std(time))
        }

        let root_dir = self
            .root_dir
            .try_clone()
            .into_io_error("failed to get root directory handle")?;
        let path = path.to_owned();
        let atime = atime.clone().map(convert_system_time);
        let mtime = mtime.clone().map(convert_system_time);

        tokio::task::spawn_blocking(move || {
            root_dir
                .set_times(path, atime, mtime)
                .into_io_error("failed to set times")
        })
        .await
        .unwrap_or_else(|e| {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }

            panic!("task failed: {e}");
        })?;

        Ok(())
    }

    async fn set_times_fd(
        &self,
        handle: &Handle,
        atime: Option<SystemTime>,
        mtime: Option<SystemTime>,
    ) -> Result<(), Error> {
        if handle.handle_type() == HandleType::File {
            use fs_set_times::{SetTimes, SystemTimeSpec};

            let file = self.get_file(handle).await?;
            let atime = atime.map(SystemTimeSpec::Absolute);
            let mtime = mtime.map(SystemTimeSpec::Absolute);

            tokio::task::spawn_blocking(move || {
                file.set_times(atime, mtime)
                    .into_io_error("failed to set times")
            })
            .await
            .unwrap_or_else(|e| {
                if e.is_panic() {
                    std::panic::resume_unwind(e.into_panic());
                }

                panic!("task failed: {e}");
            })?;

            Ok(())
        } else {
            Err(Error::NotAFile)
        }
    }
}
