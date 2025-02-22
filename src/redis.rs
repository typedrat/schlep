use fred::{prelude::*, types::config::Config as RedisConfig};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_inline_default::serde_inline_default;
use tracing::Level;
use url::Url;

#[serde_inline_default]
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename = "redis_config")]
pub struct Config {
    /// A connection URL for the Redis server to connect to.
    url: Url,

    /// How many connections to keep in the connection pool.
    #[serde_inline_default(10)]
    pool_size: usize,
}

pub type RedisPool = Pool;
pub type RedisError = Error;

impl Config {
    pub fn get_pool(&self) -> Result<RedisPool, RedisError> {
        let mut config = RedisConfig::from_url(self.url.as_str())?;
        config.tracing = TracingConfig {
            enabled: true,
            default_tracing_level: Level::INFO,
        };

        let builder = Builder::from_config(config);

        builder.build_pool(10)
    }
}
