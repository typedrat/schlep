use std::io;

use camino::FromPathError;

use crate::auth;

#[derive(Debug, thiserror::Error, thiserror_ext::ContextInto)]
pub enum Error {
    #[error("authentication error")]
    AuthError(#[from] auth::AuthError),
    #[error("russh error")]
    RusshError(#[from] russh::Error),
    #[error("I/O error: {from}")]
    IoError { source: io::Error, from: String },
    #[error("error parsing path")]
    FromPathError(#[from] FromPathError),
    #[error("couldn't find channel")]
    LostChannel,
}
