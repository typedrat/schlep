use std::{
    path::Path,
    result::Result,
    str::FromStr,
    sync::{Arc, LazyLock},
};

use ahash::RandomState;
use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use russh_sftp::protocol::{
    Attrs,
    Data,
    File,
    FileAttributes,
    Handle,
    Name,
    OpenFlags,
    Status,
    StatusCode,
    Version,
};
use thiserror_ext::AsReport;
use tracing::{event, Level};
use whirlwind::{mapref::MapRef, ShardMap};

use crate::{
    vfs,
    vfs::{Metadata, PathMatch, VfsInstance, VfsSet},
};

pub struct SftpSession {
    cwd_path: Utf8PathBuf,
    vfs_set: VfsSet,
    version: Option<u32>,
    readdir_performed: ShardMap<vfs::Handle, bool, RandomState>,
}

impl SftpSession {
    pub fn new(cwd_path: Utf8PathBuf, vfs_set: VfsSet) -> Self {
        Self {
            cwd_path,
            vfs_set,
            version: None,
            readdir_performed: ShardMap::with_hasher(RandomState::default()),
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
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        path_match(&self.vfs_set, &path, async |vfs, relative_path| {
            if let Ok(handle) = vfs.open(relative_path, vfs::OpenFlags::from(pflags)).await {
                Ok(Handle {
                    id,
                    handle: handle.to_string(),
                })
            } else {
                Err(StatusCode::Failure)
            }
        })
        .await
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        handle_match(&self.vfs_set, handle, async |vfs, handle| {
            self.readdir_performed.remove(&handle).await;

            if let Ok(()) = vfs.close(handle).await {
                Ok(Status {
                    id,
                    status_code: StatusCode::Ok,
                    error_message: String::new(),
                    language_tag: String::new(),
                })
            } else {
                Err(StatusCode::Failure)
            }
        })
        .await
    }
    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        handle_match(&self.vfs_set, handle, async |vfs, handle| {
            match vfs.read(&handle, offset as usize, len as usize).await {
                Ok(Some(data)) => Ok(Data { id, data }),
                Ok(None) => Err(StatusCode::Eof),
                Err(_) => Err(StatusCode::Failure),
            }
        })
        .await
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        handle_match(&self.vfs_set, handle, async |vfs, handle| {
            if let Ok(()) = vfs.write(&handle, offset as usize, data.as_slice()).await {
                Ok(Status {
                    id,
                    status_code: StatusCode::Ok,
                    error_message: String::new(),
                    language_tag: String::new(),
                })
            } else {
                Err(StatusCode::Failure)
            }
        })
        .await
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        path_match(&self.vfs_set, &path, async |vfs, relative_path| {
            if let Ok(metadata) = vfs.stat_link(relative_path).await {
                Ok(Attrs {
                    id,
                    attrs: Metadata::into(metadata),
                })
            } else {
                Err(StatusCode::Failure)
            }
        })
        .await
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        handle_match(&self.vfs_set, handle, async |vfs, handle| {
            if let Ok(metadata) = vfs.stat_fd(&handle).await {
                Ok(Attrs {
                    id,
                    attrs: Metadata::into(metadata),
                })
            } else {
                Err(StatusCode::Failure)
            }
        })
        .await
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        path_match(&self.vfs_set, &path, async |vfs, relative_path| {
            if let Ok(dir_handle) = vfs.open_dir(relative_path).await {
                Ok(Handle {
                    id,
                    handle: dir_handle.to_string(),
                })
            } else {
                Err(StatusCode::Failure)
            }
        })
        .await
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        handle_match(&self.vfs_set, handle, async |vfs, handle| {
            let readdir_performed = self
                .readdir_performed
                .get(&handle)
                .await
                .map_or(false, |x| *x.value());

            if !readdir_performed {
                if let Ok(dirs) = vfs.read_dir(&handle).await {
                    let dirs = dirs
                        .iter()
                        .map(|(path, metadata)| File::new(path.as_str(), Metadata::into(*metadata)))
                        .collect();

                    self.readdir_performed.insert(handle, true).await;

                    Ok(Name { id, files: dirs })
                } else {
                    Err(StatusCode::Failure)
                }
            } else {
                Err(StatusCode::Eof)
            }
        })
        .await
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        path_match(
            &self.vfs_set,
            &filename,
            async |vfs, relative_path| match vfs.remove_file(relative_path).await {
                Ok(()) => Ok(Status {
                    id,
                    status_code: StatusCode::Ok,
                    error_message: String::new(),
                    language_tag: String::new(),
                }),
                Err(err) => Ok(Status {
                    id,
                    status_code: StatusCode::Failure,
                    error_message: err.as_report().to_string(),
                    language_tag: LANGUAGE_TAG.clone(),
                }),
            },
        )
        .await
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        path_match(&self.vfs_set, &path, async |vfs, relative_path| {
            match vfs.remove_dir(relative_path).await {
                Ok(()) => Ok(Status {
                    id,
                    status_code: StatusCode::Ok,
                    error_message: String::new(),
                    language_tag: String::new(),
                }),
                Err(err) => Ok(Status {
                    id,
                    status_code: StatusCode::Failure,
                    error_message: err.as_report().to_string(),
                    language_tag: LANGUAGE_TAG.clone(),
                }),
            }
        })
        .await
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        use path_absolutize::Absolutize;

        let path = Path::new(&path)
            .absolutize_from(self.cwd_path.as_std_path())
            .map_err(|_| StatusCode::Failure)?
            .to_str()
            .map(|s| s.to_string())
            .ok_or(StatusCode::Failure)?;

        Ok(Name {
            id,
            files: vec![File::dummy(path)],
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        path_match(&self.vfs_set, &path, async |vfs, relative_path| {
            if let Ok(metadata) = vfs.stat(relative_path).await {
                Ok(Attrs {
                    id,
                    attrs: Metadata::into(metadata),
                })
            } else {
                Err(StatusCode::Failure)
            }
        })
        .await
    }

    async fn readlink(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let id = id;

        path_match(&self.vfs_set, &path, async |vfs, relative_path| {
            if let Ok(link_contents) = vfs.readlink(relative_path).await {
                Ok(Name {
                    id,
                    files: vec![File::dummy(link_contents)],
                })
            } else {
                Err(StatusCode::Failure)
            }
        })
        .await
    }
}

async fn handle_match<T, F>(vfs_set: &VfsSet, handle: String, fun: F) -> Result<T, StatusCode>
where
    F: AsyncFnOnce(Arc<VfsInstance>, vfs::Handle) -> Result<T, StatusCode>,
{
    let handle = vfs::Handle::from_str(&handle).map_err(|_| StatusCode::BadMessage)?;

    if let Some(vfs) = vfs_set.resolve_handle(&handle).await {
        fun(vfs, handle).await
    } else {
        Err(StatusCode::NoSuchFile)
    }
}

async fn path_match<T, F>(vfs_set: &VfsSet, path: &str, fun: F) -> Result<T, StatusCode>
where
    F: AsyncFnOnce(Arc<VfsInstance>, &Utf8Path) -> Result<T, StatusCode>,
{
    if let Some(PathMatch { vfs, relative_path }) = vfs_set.resolve_path(Utf8Path::new(path)) {
        fun(vfs, relative_path.as_path()).await
    } else {
        Err(StatusCode::NoSuchFile)
    }
}

static LANGUAGE_TAG: LazyLock<String> = LazyLock::new(|| "en".to_string());
