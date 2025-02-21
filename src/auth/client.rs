use deadpool::{
    Runtime,
    managed::{self, PoolError},
};
use ldap3::{Scope, SearchEntry, ldap_escape};
use redis::AsyncCommands;
use redis_macros::{FromRedisValue, ToRedisArgs};
use russh::keys::PublicKey;
use serde::{Deserialize, Serialize};
use tracing::{Level, event, instrument};

use super::{
    AuthError,
    Config,
    config::{LdapConfig, LdapConnectionManager},
};
use crate::{
    auth::error::{IntoLdapError, IntoRedisError},
    redis::RedisPool,
};

#[derive(Clone)]
pub struct AuthClient {
    redis_pool: Option<RedisPool>,
    ldap_config: LdapConfig,
    ldap_pool: managed::Pool<LdapConnectionManager>,
}

pub type Result<T> = std::result::Result<T, AuthError>;

impl AuthClient {
    pub fn new(config: Config, redis_pool: Option<RedisPool>) -> Result<Self> {
        let ldap_manager = config.ldap.connection_manager();
        let ldap_pool_timeout = config.ldap.conn_timeout;
        let ldap_pool_max_size = config.ldap.pool_max_size;

        let ldap_pool = managed::Pool::builder(ldap_manager)
            .runtime(Runtime::Tokio1)
            .create_timeout(Some(ldap_pool_timeout))
            .max_size(ldap_pool_max_size)
            .build()
            .unwrap();

        Ok(Self {
            redis_pool,
            ldap_config: config.ldap,
            ldap_pool,
        })
    }

    #[instrument(skip_all, err)]
    async fn read_user_cache(&self, cache_key: &str) -> Result<Option<UserInfo>> {
        if let Some(redis_pool) = self.redis_pool.as_ref() {
            let mut conn = match redis_pool.get().await {
                Ok(conn) => conn,
                Err(PoolError::Timeout(_)) => return Err(AuthError::RedisConnectionTimeout),
                Err(PoolError::Backend(err)) => {
                    return Err(err.into_redis_error("failed to get connection"));
                }
                Err(PoolError::PostCreateHook(err)) => return Err(AuthError::from(err)),
                Err(_) => return Ok(None),
            };

            match conn.get::<_, Option<UserInfo>>(&cache_key).await {
                Ok(Some(user)) => {
                    event!(Level::INFO, "successfully got user from cache");
                    Ok(Some(user))
                }
                Ok(None) => {
                    event!(Level::INFO, "didn't find user info in cache");
                    Ok(None)
                }
                Err(err) => Err(err.into_redis_error("failed to read user cache")),
            }
        } else {
            Ok(None)
        }
    }

    #[instrument(skip_all, err)]
    async fn write_user_cache(&self, cache_key: &str, user: &UserInfo) -> Result<()> {
        if let Some(redis_pool) = self.redis_pool.as_ref() {
            let mut conn = match redis_pool.get().await {
                Ok(conn) => conn,
                Err(PoolError::Timeout(_)) => return Err(AuthError::RedisConnectionTimeout),
                Err(PoolError::Backend(err)) => {
                    return Err(err.into_redis_error("failed to get connection"));
                }
                Err(PoolError::PostCreateHook(err)) => return Err(AuthError::from(err)),
                Err(_) => return Ok(()),
            };

            conn.set::<_, _, ()>(&cache_key, &user)
                .await
                .into_redis_error("failed to set LDAP user cache data")?;
            conn.expire::<_, ()>(&cache_key, 5 * 60)
                .await
                .into_redis_error("failed to set LDAP user cache expiration")?;
        }

        Ok(())
    }

    #[instrument(skip(self), err)]
    async fn get_user(&self, username: &str) -> Result<Option<UserInfo>> {
        let cache_key = format!("ldap_cache_user_{username}");

        if let Some(cached_user) = self.read_user_cache(&cache_key).await? {
            return Ok(Some(cached_user));
        }

        let mut conn = match self.ldap_pool.get().await {
            Ok(conn) => Ok(conn),
            Err(PoolError::Timeout(_)) => Err(AuthError::RedisConnectionTimeout),
            Err(PoolError::Backend(err)) => Err(err.into_ldap_error("failed to get connection")),
            Err(PoolError::PostCreateHook(err)) => Err(AuthError::from(err)),
            Err(PoolError::Closed) => Err(AuthError::LdapPoolClosed),
            Err(PoolError::NoRuntimeSpecified) => unreachable!(),
        }?;

        conn.simple_bind(&self.ldap_config.bind_dn, &self.ldap_config.bind_password)
            .await
            .into_ldap_error("failed to bind with provided bind credentials")?;

        let filter = format!(
            "{key}={value}",
            key = ldap_escape(&self.ldap_config.user_attribute),
            value = ldap_escape(username)
        );

        let search = conn
            .search(
                &self.ldap_config.base_dn,
                Scope::Subtree,
                &filter,
                vec!["dn", "memberOf", &self.ldap_config.ssh_key_attribute],
            )
            .await
            .into_ldap_error("failed to search for user")?;

        let (entries, _) = search
            .success()
            .into_ldap_error("failed to get search results")?;

        match entries.len() {
            0 => {
                event!(Level::DEBUG, username, "LDAP user not found");
                Ok(None)
            }
            1 => {
                event!(Level::DEBUG, username, "LDAP user found");
                let result = SearchEntry::construct(entries[0].clone());

                let dn = result.dn;
                let mut public_keys = Vec::new();

                if let Some(keys) = result.attrs.get(&self.ldap_config.ssh_key_attribute) {
                    for key in keys {
                        public_keys
                            .push(PublicKey::from_openssh(key).map_err(russh::keys::Error::from)?);
                    }
                }

                let user = UserInfo {
                    username: username.to_string(),
                    dn,
                    public_keys,
                };

                self.write_user_cache(&cache_key, &user).await?;

                Ok(Some(user))
            }
            _ => {
                event!(Level::WARN, username, "Multiple LDAP users found");
                Err(AuthError::MultipleUsersFound(username.to_string()))
            }
        }
    }

    #[instrument(skip(self, _password), err)]
    pub async fn authenticate_password(&self, username: &str, _password: &str) -> Result<bool> {
        todo!()
    }

    #[instrument(skip(self, key))]
    pub async fn authenticate_public_key(&self, username: &str, key: &PublicKey) -> Result<bool> {
        if let Some(user) = self.get_user(username).await? {
            Ok(user
                .public_keys
                .iter()
                .any(|pk| pk.key_data() == key.key_data()))
        } else {
            Ok(false)
        }
    }
}

#[derive(Debug, Serialize, Deserialize, FromRedisValue, ToRedisArgs)]
struct UserInfo {
    username: String,
    dn: String,
    public_keys: Vec<PublicKey>,
}
