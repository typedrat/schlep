use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::PathBuf,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_inline_default::serde_inline_default;

#[serde_inline_default]
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename = "sftp_config")]
pub struct Config {
    /// The address for the SFTP sftp to listen on.
    #[serde(default = "Config::default_address")]
    pub address: Vec<IpAddr>,

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

impl Config {
    fn default_address() -> Vec<IpAddr> {
        vec![
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)),
        ]
    }
}
