use std::{ffi::OsString, path::Path};

use camino::{Utf8Path, Utf8PathBuf};
use path_absolutize::Absolutize;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::vfs::{PathMatch, VfsSet};

pub async fn exec_sha1sum<S>(
    vfs_set: VfsSet,
    cwd: Utf8PathBuf,
    mut stream: S,
    arguments: Vec<OsString>,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    for argument in arguments {
        let path = Path::new(&argument);
        let absolute_path = path.absolutize_from(cwd.as_path())?;
        let absolute_path =
            Utf8Path::from_path(&absolute_path).ok_or(anyhow::anyhow!("invalid path"))?;

        if let Some(PathMatch { vfs, relative_path }) = vfs_set.resolve_path(absolute_path) {
            let digest = vfs.sha1sum(&relative_path).await?;
            let output_line = format!("{digest:x}  {}\n", path.display());

            stream.write_all(output_line.as_bytes()).await?;
        }
    }

    Ok(())
}

pub async fn exec_md5sum<S>(
    vfs_set: VfsSet,
    cwd: Utf8PathBuf,
    mut stream: S,
    arguments: Vec<OsString>,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    for argument in arguments {
        let path = Path::new(&argument);
        let absolute_path = path.absolutize_from(cwd.as_path())?;
        let absolute_path =
            Utf8Path::from_path(&absolute_path).ok_or(anyhow::anyhow!("invalid path"))?;

        if let Some(PathMatch { vfs, relative_path }) = vfs_set.resolve_path(absolute_path) {
            let digest = vfs.md5sum(&relative_path).await?;
            let output_line = format!("{digest:x}  {}\n", path.display());

            stream.write_all(output_line.as_bytes()).await?;
        }
    }

    Ok(())
}
