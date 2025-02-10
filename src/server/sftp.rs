use crate::config::Config;
use crate::vfs;
use ahash::RandomState;
use async_trait::async_trait;
use base64ct::{Base64, Encoding};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use rustix::fs::{AtFlags, Mode, OFlags, RawDir, ResolveFlags, Stat, StatExt};
use rustix::io::Errno;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs;
use std::io::SeekFrom;
use std::os::fd::OwnedFd;
use std::os::unix::prelude::OsStrExt;
use std::path::{Path, PathBuf};
use std::result::Result;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tracing::{event, Level};
use whirlwind::ShardMap;

pub struct SftpSession {
    config: Config,
    fs: vfs::Root,
    version: Option<u32>,
    open_files: ShardMap<String, (PathBuf, OwnedFd), RandomState>,
    open_dirs: ShardMap<String, (PathBuf, OwnedFd), RandomState>,
}

impl SftpSession {
    pub fn new(config: Config, fs: vfs::Root) -> Self {
        Self {
            config,
            fs,
            version: None,
            open_files: ShardMap::with_hasher(Default::default()),
            open_dirs: ShardMap::with_hasher(Default::default()),
        }
    }
}

#[async_trait]
impl russh_sftp::server::Handler for SftpSession {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        version: u32,
        _extensions: std::collections::HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        if self.version.is_some() {
            event!(
                Level::ERROR,
                new_version = version,
                old_version = self.version,
                "Tried to negotiate version after initial handshake"
            );
            Err(StatusCode::BadMessage)
        } else {
            self.version = Some(version);
            Ok(Version::new())
        }
    }

    async fn open(
        &mut self,
        id: u32,
        path: String,
        pflags: OpenFlags,
        attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let handle_hash = Sha256::digest(path.as_bytes());
        let handle = format!("file_{}", Base64::encode_string(handle_hash.as_slice()));
        let path = PathBuf::from(path);

        let mut oflags = OFlags::CLOEXEC;
        let mode = attrs.permissions.map_or(Mode::empty(), Mode::from);

        if pflags.contains(OpenFlags::READ) && pflags.contains(OpenFlags::WRITE) {
            oflags |= OFlags::RDWR
        } else if pflags.contains(OpenFlags::READ) {
            oflags |= OFlags::RDONLY
        } else if pflags.contains(OpenFlags::WRITE) {
            oflags |= OFlags::WRONLY
        }

        if pflags.contains(OpenFlags::APPEND) {
            oflags |= OFlags::APPEND
        }

        if pflags.contains(OpenFlags::CREATE) {
            oflags |= OFlags::CREATE
        }

        if pflags.contains(OpenFlags::TRUNCATE) {
            oflags |= OFlags::TRUNC
        }

        if pflags.contains(OpenFlags::EXCLUDE) {
            oflags |= OFlags::EXCL
        }

        let dir_fd = self
            .fs
            .open_fd(&path, oflags, mode)
            .map_err(|_| StatusCode::Failure)?;

        self.open_files.insert(handle.clone(), (path, dir_fd)).await;

        Ok(Handle { id, handle })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        if handle.starts_with("dir") {
            self.open_dirs.remove(&handle).await;
            Ok(Status {
                id,
                status_code: StatusCode::Ok,
                error_message: String::new(),
                language_tag: String::new(),
            })
        } else if handle.starts_with("file") {
            self.open_files.remove(&handle).await;
            Ok(Status {
                id,
                status_code: StatusCode::Ok,
                error_message: String::new(),
                language_tag: String::new(),
            })
        } else {
            Err(StatusCode::Failure)
        }
    }
    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let owned_fd = if let Some(map_ref) = self.open_files.get(&handle).await {
            let (_, file_fd) = map_ref.value();
            let owned_fd = file_fd.try_clone().map_err(|_| StatusCode::Failure)?;

            owned_fd
        } else {
            return Err(StatusCode::Failure);
        };
        let mut owned_file = tokio::fs::File::from(std::fs::File::from(owned_fd));

        owned_file
            .seek(SeekFrom::Start(offset))
            .await
            .map_err(|_| StatusCode::Failure)?;

        let mut data = Vec::with_capacity(len as usize);
        let n = owned_file
            .read_buf(&mut data)
            .await
            .map_err(|_| StatusCode::Failure)?;

        if n > 0 {
            Ok(Data { id, data })
        } else {
            Err(StatusCode::Eof)
        }
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let owned_fd = if let Some(map_ref) = self.open_files.get(&handle).await {
            let (_, file_fd) = map_ref.value();
            let owned_fd = file_fd.try_clone().map_err(|_| StatusCode::Failure)?;

            owned_fd
        } else {
            return Err(StatusCode::Failure);
        };
        let mut owned_file = tokio::fs::File::from(std::fs::File::from(owned_fd));

        owned_file
            .seek(SeekFrom::Start(offset))
            .await
            .map_err(|_| StatusCode::Failure)?;

        owned_file
            .write_all(data.as_slice())
            .await
            .map_err(|_| StatusCode::Failure)?;

        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: String::new(),
            language_tag: String::new(),
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let path = PathBuf::from(path);

        let fd = self
            .fs
            .open_fd(
                &path,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| StatusCode::Failure)?;
        let stat = rustix::fs::fstat(fd).map_err(|_| StatusCode::Failure)?;

        Ok(Attrs {
            id,
            attrs: stat_to_attrs(&stat),
        })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        if let Some(map_ref) = self.open_files.get(&handle).await {
            let (_, fd) = map_ref.value();
            let stat = rustix::fs::fstat(fd).map_err(|_| StatusCode::Failure)?;

            Ok(Attrs {
                id,
                attrs: stat_to_attrs(&stat),
            })
        } else {
            Err(StatusCode::Failure)
        }
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let handle_hash = Sha256::digest(path.as_bytes());
        let handle = format!("dir_{}", Base64::encode_string(handle_hash.as_slice()));
        let path = PathBuf::from(path);

        let dir_fd = self
            .fs
            .open_fd(
                &path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| StatusCode::Failure)?;

        self.open_dirs.insert(handle.clone(), (path, dir_fd)).await;

        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        if let Some(map_ref) = self.open_dirs.get(&handle).await {
            let (path, dir_fd) = map_ref.value();
            let mut files = Vec::new();

            let mut buf = Vec::with_capacity(8192);
            'read: loop {
                'resize: {
                    let mut iter = RawDir::new(dir_fd, buf.spare_capacity_mut());

                    while let Some(entry) = iter.next() {
                        let entry = match entry {
                            Ok(entry) => entry,
                            Err(Errno::INVAL) => break 'resize,
                            Err(_) => return Err(StatusCode::Failure),
                        };

                        let filename_cstr = OsStr::from_bytes(entry.file_name().to_bytes());
                        if filename_cstr == "." || filename_cstr == ".." {
                            continue;
                        }

                        let filename = Path::new(filename_cstr);
                        let file_fd = match self.fs.open_fd(
                            &*path.join(filename),
                            OFlags::RDONLY | OFlags::CLOEXEC,
                            Mode::empty(),
                        ) {
                            Ok(fd) => fd,
                            Err(_) => continue,
                        };

                        let file_stat = match rustix::fs::fstat(file_fd) {
                            Ok(stat) => stat,
                            Err(_) => continue,
                        };

                        files.push(File::new(
                            filename_cstr.to_string_lossy(),
                            stat_to_attrs(&file_stat),
                        ))
                    }

                    break 'read;
                }
            }

            Ok(Name { id, files })
        } else {
            Err(StatusCode::Failure)
        }
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let _ = self
            .fs
            .remove(Path::new(&filename))
            .map_err(|_| StatusCode::Failure)?;

        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: String::new(),
            language_tag: String::new(),
        })
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        let _ = self
            .fs
            .remove(Path::new(&path))
            .map_err(|_| StatusCode::Failure)?;

        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: String::new(),
            language_tag: String::new(),
        })
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        if let Ok(resolved) = self.fs.absolute_path(Path::new(&path)) {
            let resolved_str = resolved.to_str().unwrap();

            Ok(Name {
                id,
                files: vec![File::dummy(resolved_str)],
            })
        } else {
            Err(StatusCode::Failure)
        }
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let path = PathBuf::from(path);

        let fd = self
            .fs
            .open_fd(&path, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())
            .map_err(|_| StatusCode::Failure)?;
        let stat = rustix::fs::fstat(fd).map_err(|_| StatusCode::Failure)?;

        Ok(Attrs {
            id,
            attrs: stat_to_attrs(&stat),
        })
    }

    async fn readlink(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        if let Ok(resolved) = self.fs.readlink(Path::new(&path)) {
            let resolved_str = resolved.to_str().unwrap();

            Ok(Name {
                id,
                files: vec![File::dummy(resolved_str)],
            })
        } else {
            Err(StatusCode::Failure)
        }
    }
}

fn stat_to_attrs(stat: &Stat) -> FileAttributes {
    use users::{get_group_by_gid, get_user_by_uid};

    let mut attrs = FileAttributes::default();

    attrs.size = Some(stat.st_size as u64);
    attrs.uid = Some(stat.st_uid);
    attrs.user =
        get_user_by_uid(stat.st_uid).and_then(|user| user.name().to_str().map(String::from));
    attrs.gid = Some(stat.st_gid);
    attrs.group =
        get_group_by_gid(stat.st_gid).and_then(|group| group.name().to_str().map(String::from));
    attrs.permissions = Some(stat.st_mode);
    attrs.atime = Some(stat.st_atime as u32);
    attrs.mtime = Some(stat.st_mtime as u32);

    attrs
}
