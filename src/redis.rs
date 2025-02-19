#![allow(dead_code)]
use std::{path::PathBuf, time::Duration};

use deadpool::{managed::QueueMode, Runtime};
use deadpool_redis::{
    ConnectionAddr,
    ConnectionInfo,
    CreatePoolError,
    PoolConfig,
    ProtocolVersion,
    RedisConnectionInfo,
    Timeouts,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[repr(transparent)]
#[serde(rename = "redis_config", transparent)]
pub struct Config {
    /// A connection URL for the Redis server to connect to.
    #[schemars(with = "RedisConfigSchema")]
    config: deadpool_redis::Config,
}

pub type RedisPool = deadpool_redis::Pool;

impl Config {
    pub fn get_pool(&self) -> Result<RedisPool, CreatePoolError> {
        self.config.create_pool(Some(Runtime::Tokio1))
    }
}

//

#[derive(JsonSchema)]
struct RedisConfigSchema {
    pub url: Option<String>,
    #[schemars(with = "Option::<ConnectionInfoSchema>")]
    pub connection: Option<ConnectionInfo>,
    #[schemars(with = "Option::<PoolConfigSchema>")]
    pub pool: Option<PoolConfig>,
}

#[derive(JsonSchema)]
struct ConnectionInfoSchema {
    #[schemars(with = "ConnectionAddrSchema")]
    pub addr: ConnectionAddr,
    #[schemars(with = "RedisConnectionInfoSchema")]
    pub redis: RedisConnectionInfo,
}

#[derive(JsonSchema)]
enum ConnectionAddrSchema {
    Tcp(String, u16),
    TcpTls {
        host: String,
        port: u16,
        insecure: bool,
    },
    Unix(PathBuf),
}

#[derive(JsonSchema)]
struct RedisConnectionInfoSchema {
    pub db: i64,
    pub username: Option<String>,
    pub password: Option<String>,
    #[schemars(with = "ProtocolVersionSchema")]
    pub protocol: ProtocolVersion,
}

#[derive(JsonSchema)]
enum ProtocolVersionSchema {
    RESP2,
    RESP3,
}

#[derive(JsonSchema)]
struct PoolConfigSchema {
    pub max_size: usize,
    #[schemars(with = "TimeoutsSchema")]
    pub timeouts: Timeouts,
    #[schemars(with = "QueueModeSchema")]
    pub queue_mode: QueueMode,
}

#[derive(JsonSchema)]
pub struct TimeoutsSchema {
    pub wait: Option<Duration>,
    pub create: Option<Duration>,
    pub recycle: Option<Duration>,
}

#[derive(JsonSchema)]
enum QueueModeSchema {
    Fifo,
    Lifo,
}
