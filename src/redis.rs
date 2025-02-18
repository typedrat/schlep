use bb8::Pool;
use bb8_redis::RedisConnectionManager;
use redis::RedisError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename = "redis_config")]
pub struct Config {
    /// A connection URL for the Redis server to connect to.
    url: Url,
}

pub type RedisPool = Pool<RedisConnectionManager>;

impl Config {
    pub async fn get_pool(&self) -> Result<RedisPool, RedisError> {
        let manager = RedisConnectionManager::new(self.url.clone())?;

        Pool::builder().build(manager).await
    }
}
