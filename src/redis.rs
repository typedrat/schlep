use bb8::Pool;
use bb8_redis::RedisConnectionManager;
use redis::RedisError;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    url: Url,
}

pub type RedisPool = Pool<RedisConnectionManager>;

impl Config {
    pub async fn get_pool(&self) -> Result<RedisPool, RedisError> {
        let manager = RedisConnectionManager::new(self.url.clone())?;

        Pool::builder().build(manager).await
    }
}
