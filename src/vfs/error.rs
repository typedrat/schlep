use std::path::PathBuf;

#[derive(thiserror::Error, thiserror_ext::ContextInto, Debug)]
#[non_exhaustive]
pub enum Error {
    #[error("i/o error: {from}")]
    IoError {
        source: std::io::Error,
        from: String,
    },
    #[error("invalid path: {0}")]
    InvalidPath(PathBuf),
    #[error("unsupported method")]
    UnsupportedMethod,
    #[error("not a file")]
    NotAFile,
    #[error("not a directory")]
    NotADirectory,
    #[error("file not found")]
    FileNotFound,
    #[error("would escape VFS root")]
    WouldEscape,
}
