use std::{
    collections::HashMap,
    path::Path,
    result::Result,
    str::FromStr,
    sync::{Arc, LazyLock},
    time::{Duration, SystemTime},
};

use ahash::RandomState;
use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use metrics::histogram;
use path_absolutize::Absolutize;
use russh_sftp::protocol::{
    Attrs,
    Data,
    File,
    FileAttributes,
    Handle,
    Name,
    OpenFlags,
    Packet,
    Status,
    StatusCode,
    Version,
};
use thiserror_ext::AsReport;
use tracing::{event, instrument, Level};
use whirlwind::ShardSet;

use super::Config;
use crate::{
    metrics::Metrics,
    vfs,
    vfs::{PathMatch, VfsInstance, VfsSet},
};

pub struct SftpSession {
    config: Config,
    authenticated_username: String,
    cwd_path: Utf8PathBuf,
    vfs_set: VfsSet,
    version: Option<u32>,
    readdir_performed: ShardSet<vfs::Handle, RandomState>,
}

impl SftpSession {
    pub fn new(
        config: Config,
        authenticated_username: String,
        cwd_path: Utf8PathBuf,
        vfs_set: VfsSet,
    ) -> Self {
        Self {
            config,
            authenticated_username,
            cwd_path,
            vfs_set,
            version: None,
            readdir_performed: ShardSet::new_with_hasher(RandomState::default()),
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
        _extensions: HashMap<String, String>,
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

            Ok(Version {
                version,
                extensions: HashMap::new(),
            })
        }
    }

    async fn open(
        &mut self,
        id: u32,
        path: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        path_match(
            &self.vfs_set,
            &self.cwd_path,
            &path,
            async |vfs, relative_path| {
                if let Ok(handle) = vfs.open(relative_path, vfs::OpenFlags::from(pflags)).await {
                    Ok(Handle {
                        id,
                        handle: handle.to_string(),
                    })
                } else {
                    Err(StatusCode::Failure)
                }
            },
        )
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

    #[instrument(skip_all, fields(size = len, vfs))]
    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let start_time = SystemTime::now();

        let data = handle_match(&self.vfs_set, handle, async |vfs, handle| {
            tracing::Span::current().record("vfs", &vfs.vfs_root().as_str());

            match vfs.read(&handle, offset as usize, len as usize).await {
                Ok(Some(data)) => Ok(Data { id, data }),
                Ok(None) => Err(StatusCode::Eof),
                Err(_) => Err(StatusCode::Failure),
            }
        })
        .await?;

        let end_time = SystemTime::now();
        if let Ok(duration) = end_time.duration_since(start_time) {
            histogram!(Metrics::SFTP_READ_DURATION).record(duration);
        }

        Ok(data)
    }

    #[instrument(skip_all, fields(size = data.len(), vfs))]
    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let start_time = SystemTime::now();

        let status = handle_match(&self.vfs_set, handle, async |vfs, handle| {
            tracing::Span::current().record("vfs", &vfs.vfs_root().as_str());

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
        .await?;

        let end_time = SystemTime::now();
        if let Ok(duration) = end_time.duration_since(start_time) {
            histogram!(Metrics::SFTP_WRITE_DURATION).record(duration);
        }

        Ok(status)
    }
    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        path_match(
            &self.vfs_set,
            &self.cwd_path,
            &path,
            async |vfs, relative_path| {
                if let Ok(metadata) = vfs.stat_link(relative_path).await {
                    Ok(Attrs {
                        id,
                        attrs: metadata.file_attrs(
                            self.config.default_file_mode,
                            self.config.default_dir_mode,
                        ),
                    })
                } else {
                    Err(StatusCode::Failure)
                }
            },
        )
        .await
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        handle_match(&self.vfs_set, handle, async |vfs, handle| {
            if let Ok(metadata) = vfs.stat_fd(&handle).await {
                Ok(Attrs {
                    id,
                    attrs: metadata
                        .file_attrs(self.config.default_file_mode, self.config.default_dir_mode),
                })
            } else {
                Err(StatusCode::Failure)
            }
        })
        .await
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        path_match(
            &self.vfs_set,
            &self.cwd_path,
            &path,
            async |vfs, relative_path| {
                let atime = attrs.atime.map(to_system_time);
                let mtime = attrs.mtime.map(to_system_time);

                vfs.set_times(relative_path, atime, mtime)
                    .await
                    .map_err(|_| StatusCode::Failure)
            },
        )
        .await
        .map(|()| Status {
            id,
            status_code: StatusCode::Ok,
            error_message: String::new(),
            language_tag: String::new(),
        })
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        handle: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        handle_match(&self.vfs_set, handle, async |vfs, handle| {
            let atime = attrs.atime.map(to_system_time);
            let mtime = attrs.mtime.map(to_system_time);

            vfs.set_times_fd(&handle, atime, mtime)
                .await
                .map_err(|_| StatusCode::Failure)
        })
        .await
        .map(|()| Status {
            id,
            status_code: StatusCode::Ok,
            error_message: String::new(),
            language_tag: String::new(),
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        path_match(
            &self.vfs_set,
            &self.cwd_path,
            &path,
            async |vfs, relative_path| {
                if let Ok(dir_handle) = vfs.open_dir(relative_path).await {
                    Ok(Handle {
                        id,
                        handle: dir_handle.to_string(),
                    })
                } else {
                    Err(StatusCode::Failure)
                }
            },
        )
        .await
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        handle_match(&self.vfs_set, handle, async |vfs, handle| {
            let readdir_performed = self.readdir_performed.contains(&handle).await;

            if !readdir_performed {
                if let Ok(dirs) = vfs.read_dir(&handle).await {
                    let dirs = dirs
                        .iter()
                        .map(|(path, metadata)| {
                            File::new(
                                path.as_str(),
                                metadata.file_attrs(
                                    self.config.default_file_mode,
                                    self.config.default_dir_mode,
                                ),
                            )
                        })
                        .collect();

                    self.readdir_performed.insert(handle).await;

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
            &self.cwd_path,
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

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        path_match(
            &self.vfs_set,
            &self.cwd_path,
            &path,
            async |vfs, relative_path| match vfs.mkdir(relative_path).await {
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
        path_match(
            &self.vfs_set,
            &self.cwd_path,
            &path,
            async |vfs, relative_path| match vfs.remove_dir(relative_path).await {
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
        path_match(
            &self.vfs_set,
            &self.cwd_path,
            &path,
            async |vfs, relative_path| {
                if let Ok(metadata) = vfs.stat(relative_path).await {
                    Ok(Attrs {
                        id,
                        attrs: metadata.file_attrs(
                            self.config.default_file_mode,
                            self.config.default_dir_mode,
                        ),
                    })
                } else {
                    Err(StatusCode::Failure)
                }
            },
        )
        .await
    }

    async fn rename(
        &mut self,
        id: u32,
        old_path: String,
        new_path: String,
    ) -> Result<Status, Self::Error> {
        path_match2(
            &self.vfs_set,
            &self.cwd_path,
            &old_path,
            &new_path,
            async move |vfs, path1, path2| {
                if vfs.rename(path1, path2).await.is_ok() {
                    Ok(Status {
                        id,
                        status_code: StatusCode::Ok,
                        error_message: String::new(),
                        language_tag: String::new(),
                    })
                } else {
                    Err(StatusCode::Failure)
                }
            },
        )
        .await
    }

    async fn readlink(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let id = id;

        path_match(
            &self.vfs_set,
            &self.cwd_path,
            &path,
            async |vfs, relative_path| {
                if let Ok(link_contents) = vfs.readlink(relative_path).await {
                    Ok(Name {
                        id,
                        files: vec![File::dummy(link_contents)],
                    })
                } else {
                    Err(StatusCode::Failure)
                }
            },
        )
        .await
    }

    async fn symlink(
        &mut self,
        id: u32,
        link_path: String,
        target_path: String,
    ) -> Result<Status, Self::Error> {
        path_match2(
            &self.vfs_set,
            &self.cwd_path,
            &link_path,
            &target_path,
            async move |vfs, path1, path2| {
                if vfs.symlink(path1, path2).await.is_ok() {
                    Ok(Status {
                        id,
                        status_code: StatusCode::Ok,
                        error_message: String::new(),
                        language_tag: String::new(),
                    })
                } else {
                    Err(StatusCode::Failure)
                }
            },
        )
        .await
    }

    async fn extended(
        &mut self,
        _id: u32,
        request: String,
        _data: Vec<u8>,
    ) -> Result<Packet, Self::Error> {
        match request.as_str() {
            _ => Err(StatusCode::OpUnsupported),
        }
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

async fn path_match<T, F>(
    vfs_set: &VfsSet,
    cwd: &Utf8Path,
    path: &str,
    fun: F,
) -> Result<T, StatusCode>
where
    F: AsyncFnOnce(Arc<VfsInstance>, &Utf8Path) -> Result<T, StatusCode>,
{
    let path = Path::new(path);
    if let Ok(Some(absolute_path)) = path.absolutize_from(cwd.as_std_path()).map(|p| {
        Utf8Path::from_path(&*p)
            .to_owned()
            .map(Utf8Path::to_path_buf)
    }) {
        if let Some(PathMatch { vfs, relative_path }) = vfs_set.resolve_path(&absolute_path) {
            fun(vfs, relative_path.as_path()).await
        } else {
            Err(StatusCode::NoSuchFile)
        }
    } else {
        Err(StatusCode::Failure)
    }
}

async fn path_match2<T, F>(
    vfs_set: &VfsSet,
    cwd: &Utf8Path,
    path1: &str,
    path2: &str,
    fun: F,
) -> Result<T, StatusCode>
where
    F: AsyncFnOnce(Arc<VfsInstance>, &Utf8Path, &Utf8Path) -> Result<T, StatusCode>,
{
    let path1 = Path::new(path1);
    let path2 = Path::new(path2);

    let absolute_path1 = match path1.absolutize_from(cwd.as_std_path()).map(|p| {
        Utf8Path::from_path(&*p)
            .to_owned()
            .map(Utf8Path::to_path_buf)
    }) {
        Ok(Some(path)) => path,
        _ => return Err(StatusCode::Failure),
    };

    let absolute_path2 = match path2.absolutize_from(cwd.as_std_path()).map(|p| {
        Utf8Path::from_path(&*p)
            .to_owned()
            .map(Utf8Path::to_path_buf)
    }) {
        Ok(Some(path)) => path,
        _ => return Err(StatusCode::Failure),
    };

    let path_match1 = vfs_set.resolve_path(&absolute_path1);
    let path_match2 = vfs_set.resolve_path(&absolute_path2);

    match (path_match1, path_match2) {
        (
            Some(PathMatch {
                vfs: vfs1,
                relative_path: relative_path1,
            }),
            Some(PathMatch {
                vfs: vfs2,
                relative_path: relative_path2,
            }),
        ) => {
            if Arc::ptr_eq(&vfs1, &vfs2) {
                fun(vfs1, relative_path1.as_path(), relative_path2.as_path()).await
            } else {
                Err(StatusCode::Failure)
            }
        }
        _ => Err(StatusCode::NoSuchFile),
    }
}

fn to_system_time(epoch_secs: u32) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(u64::from(epoch_secs))
}

static LANGUAGE_TAG: LazyLock<String> = LazyLock::new(|| "en".to_string());
