use anyhow::Result;
use figment::{
    Figment,
    providers::{Env, Format, Toml},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{auth, metrics, redis, sftp, vfs};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Config {
    /// Configuration for Schlep's SFTP server.
    pub sftp: sftp::Config,

    /// Configuration for Schlep's authentication system.
    pub auth: auth::Config,

    /// An array of configuration objects defining the virtual filesystem roots.
    pub fs: vfs::Config,

    /// Configuration for a Redis-compatible cache server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redis: Option<redis::Config>,

    pub metrics: metrics::Config,
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
