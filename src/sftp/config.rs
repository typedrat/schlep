use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_inline_default::serde_inline_default;

#[serde_inline_default]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// The address for the SFTP sftp to listen on.
    #[serde_inline_default("localhost".to_string())]
    pub address: String,

    /// The port for the SFTP sftp to listen on.
    #[serde_inline_default(2222)]
    pub port: u16,

    /// Path to a directory containing OpenSSH-formatted private keys for the
    /// host to advertise to clients.
    pub private_host_key_dir: PathBuf,

    /// Allow clients to authenticate with their passwords.
    #[serde_inline_default(false)]
    pub allow_password: bool,

    /// Allow clients to authenticate with their public keys.
    #[serde_inline_default(true)]
    pub allow_publickey: bool,

    #[serde_inline_default(0o666)]
    pub default_file_mode: u32,

    #[serde_inline_default(0o777)]
    pub default_dir_mode: u32,
}
