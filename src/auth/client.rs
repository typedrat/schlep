use bb8::RunError;
use ldap3::{ldap_escape, Scope, SearchEntry};
use redis::AsyncCommands;
use redis_macros::{FromRedisValue, ToRedisArgs};
use russh::keys::PublicKey;
use serde::{Deserialize, Serialize};
use tracing::{event, instrument, Level};

use super::{config::LdapConnectionManager, AuthError, Config};
use crate::{
    auth::error::{IntoLdapError, IntoRedisError},
    redis::RedisPool,
};

#[derive(Clone)]
pub struct AuthClient {
    config: Config,
    redis_pool: Option<RedisPool>,
    ldap_pool: bb8::Pool<LdapConnectionManager>,
}

pub type Result<T> = std::result::Result<T, AuthError>;

impl AuthClient {
    pub async fn new(config: Config, redis_pool: Option<RedisPool>) -> Result<Self> {
        let ldap_manager = config.ldap.connection_manager();
        let ldap_pool_timeout = config.ldap.conn_timeout;
        let ldap_pool_max_size = config.ldap.pool_max_size;
        let ldap_pool_min_size = if config.ldap.pool_min_size > 0 {
            Some(config.ldap.pool_min_size)
        } else {
            None
        };

        let ldap_pool = bb8::Pool::builder()
            .connection_timeout(ldap_pool_timeout)
            .max_size(ldap_pool_max_size)
            .min_idle(ldap_pool_min_size)
            .build(ldap_manager)
            .await?;

        Ok(Self {
            config,
            redis_pool,
            ldap_pool,
        })
    }

    #[instrument(skip_all, err)]
    async fn read_user_cache(&self, cache_key: &str) -> Result<Option<UserInfo>> {
        if let Some(redis_pool) = self.redis_pool.as_ref() {
            let mut conn = match redis_pool.get().await {
                Ok(conn) => conn,
                Err(RunError::TimedOut) => return Err(AuthError::RedisConnectionTimeout),
                Err(RunError::User(err)) => {
                    return Err(err.into_redis_error("failed to get connection"))
                }
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
                Err(RunError::TimedOut) => return Err(AuthError::RedisConnectionTimeout),
                Err(RunError::User(err)) => {
                    return Err(err.into_redis_error("failed to get connection"))
                }
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
            Ok(conn) => conn,
            Err(RunError::TimedOut) => return Err(AuthError::LdapConnectionTimeout),
            Err(RunError::User(err)) => return Err(err),
        };

        conn.simple_bind(&self.config.ldap.bind_dn, &self.config.ldap.bind_password)
            .await
            .into_ldap_error("failed to bind with provided bind credentials")?;

        let filter = format!(
            "{key}={value}",
            key = ldap_escape(&self.config.ldap.user_attribute),
            value = ldap_escape(username)
        );

        let search = conn
            .search(
                &self.config.ldap.base_dn,
                Scope::Subtree,
                &filter,
                vec!["dn", "memberOf", &self.config.ldap.ssh_key_attribute],
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

                if let Some(keys) = result.attrs.get(&self.config.ldap.ssh_key_attribute) {
                    for key in keys {
                        public_keys
                            .push(PublicKey::from_openssh(key).map_err(russh::keys::Error::from)?)
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
