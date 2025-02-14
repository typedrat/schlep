use std::path::PathBuf;

use anyhow::Result;
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};
use serde_inline_default::serde_inline_default;

#[serde_inline_default]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SftpConfig {
    #[serde_inline_default("localhost".to_string())]
    /// The address for the SFTP server to listen on.
    pub address: String,

    #[serde_inline_default(2222)]
    /// The port for the SFTP server to listen on.
    pub port: u16,

    #[serde_inline_default(Vec::new())]
    /// Path to an OpenSSH-formatted private key for the host to advertise to
    /// clients.
    pub private_host_keys: Vec<PathBuf>,
}

#[serde_inline_default]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LdapConfig {
    /// LDAP URL to connect to for user backend.
    pub url: String,
    #[serde_inline_default(false)]
    /// Skip verifying the TLS certificate for the LDAP connection.
    pub tls_no_verify: bool,

    /// Bind user for LDAP search queries.
    pub bind_user: String,

    /// Password for the LDAP bind user.
    pub bind_password: String,

    /// Base DN for LDAP searches.
    pub base_dn: String,

    /// LDAP search filter to limit user searches.
    pub search_filter: Option<String>,

    #[serde_inline_default("cn".to_string())]
    /// LDAP attribute to search for the username provided.
    pub search_attribute: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FsConfig {
    /// The root directory to serve.
    pub root_dir: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub sftp: SftpConfig,
    pub ldap: LdapConfig,
    pub fs: FsConfig,
}

impl Config {
    pub fn load() -> Result<Config> {
        let config: Config = Figment::new()
            .merge(Toml::file("schlep.toml"))
            .merge(Env::prefixed("SCHLEP_").split("__"))
            .extract()?;

        Ok(config)
    }
}
