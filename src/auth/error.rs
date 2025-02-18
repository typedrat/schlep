use ldap3::LdapError;
use redis::RedisError;

#[derive(thiserror::Error, thiserror_ext::ContextInto, Debug)]
#[non_exhaustive]
pub enum AuthError {
    #[error("LDAP error: {from}")]
    LdapError { source: LdapError, from: String },
    #[error("redis error: {from}")]
    RedisError { source: RedisError, from: String },
    #[error("SSH key error")]
    SshKeyError(#[from] russh::keys::Error),
    #[error("not connected")]
    NotConnected,
    #[error("multiple users found")]
    MultipleUsersFound(String),
    #[error("LDAP connection timed out")]
    LdapConnectionTimeout,
    #[error("Redis connection timed out")]
    RedisConnectionTimeout,
}
