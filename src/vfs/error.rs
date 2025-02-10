use super::root::{EXPECTED_SYSTEM, EXPECTED_VERSION};
use rustix::io::Errno;
use semver::Version;
use show_option::ShowOption as _;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Unsupported operating system {found_system} (version: {}) found, {} {} is required.", .found_version.show_or("unknown"), EXPECTED_SYSTEM, &*EXPECTED_VERSION)]
    UnsupportedSystem {
        found_system: String,
        found_version: Option<Version>,
    },
    #[error("System error:")]
    SystemError(#[from] Errno),
    #[error("I/O error:")]
    IOError(#[from] std::io::Error),
    #[error("Invalid path")]
    InvalidPath(PathBuf),
}
