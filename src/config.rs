use anyhow::Result;
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{auth, redis, sftp, vfs};

#[derive(derive_more::Debug, Serialize, Deserialize, JsonSchema, Clone)]
pub struct Config {
    /// Configuration for Schlep's SFTP server.
    pub sftp: sftp::Config,

    /// Configuration for Schlep's authentication system.
    pub auth: auth::Config,

    /// An array of configuration objects defining the virtual filesystem roots.
    pub fs: vfs::Config,

    /// Configuration for a Redis-compatible cache server.
    #[serde(default)]
    pub redis: Option<redis::Config>,
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
