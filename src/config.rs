use anyhow::Result;
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};

use crate::{auth, redis, sftp};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FsConfig {
    /// The root directory to serve.
    pub root_dir: String,
}

#[derive(derive_more::Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub sftp: sftp::Config,
    pub auth: auth::Config,
    pub fs: FsConfig,
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
