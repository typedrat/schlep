use std::time::Duration;

use bb8::ManageConnection;
use ldap3::{Ldap, LdapConnAsync, LdapConnSettings};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::instrument;
use url::Url;

use super::{error::IntoLdapError, AuthError};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LdapConfig {
    /// LDAP URL to connect to for user backend.
    pub(super) url: Url,

    /// The maximum number of connections in the connection pool.
    #[serde(
        default = "LdapConfig::default_pool_max_size",
        skip_serializing_if = "LdapConfig::is_default_pool_max_size"
    )]
    pub(super) pool_max_size: u32,

    /// The minimum number of connections in the connection pool.
    #[serde(
        default = "LdapConfig::default_pool_min_size",
        skip_serializing_if = "LdapConfig::is_default_pool_min_size"
    )]
    pub(super) pool_min_size: u32,

    /// The connection timeout for the LDAP sftp. The default
    /// value is 120 seconds.
    #[serde(
        default = "LdapConfig::default_conn_timeout",
        skip_serializing_if = "LdapConfig::is_default_conn_timeout",
        with = "humantime_serde"
    )]
    #[schemars(with = "String")]
    pub(super) conn_timeout: Duration,

    /// Enable StartTLS on the LDAP connection.
    pub(super) starttls: Option<bool>,

    /// Skip verifying the TLS certificate for the LDAP connection.
    pub(super) tls_no_verify: Option<bool>,

    /// Bind DN for LDAP search queries.
    pub(super) bind_dn: String,

    /// Password for the LDAP bind user.
    pub(super) bind_password: String,

    /// Base DN for LDAP searches.
    pub(super) base_dn: String,

    /// LDAP attribute containing the username.
    #[serde(
        default = "LdapConfig::default_user_attribute",
        skip_serializing_if = "LdapConfig::is_default_user_attribute"
    )]
    pub(super) user_attribute: String,

    /// LDAP attribute containing SSH public keys.
    #[serde(
        default = "LdapConfig::default_ssh_key_attribute",
        skip_serializing_if = "LdapConfig::is_default_ssh_key_attribute"
    )]
    pub(super) ssh_key_attribute: String,
}

impl LdapConfig {
    fn default_pool_max_size() -> u32 {
        10
    }

    fn is_default_pool_max_size(size: &u32) -> bool {
        *size == Self::default_pool_max_size()
    }

    fn default_pool_min_size() -> u32 {
        0
    }

    fn is_default_pool_min_size(size: &u32) -> bool {
        *size == Self::default_pool_min_size()
    }

    fn default_conn_timeout() -> Duration {
        Duration::from_secs(120)
    }

    fn is_default_conn_timeout(timeout: &Duration) -> bool {
        *timeout == Self::default_conn_timeout()
    }

    fn default_user_attribute() -> String {
        "cn".to_string()
    }

    fn is_default_user_attribute(attribute: &String) -> bool {
        attribute.as_str() == Self::default_user_attribute()
    }

    fn default_ssh_key_attribute() -> String {
        "sshPublicKey".to_string()
    }

    fn is_default_ssh_key_attribute(attribute: &String) -> bool {
        attribute.as_str() == Self::default_ssh_key_attribute()
    }

    pub(super) fn connection_manager(&self) -> LdapConnectionManager {
        let mut conn_settings = LdapConnSettings::default();

        if let Some(starttls) = self.starttls {
            conn_settings = conn_settings.set_starttls(starttls);
        }

        if let Some(tls_no_verify) = self.tls_no_verify {
            conn_settings = conn_settings.set_no_tls_verify(tls_no_verify);
        }

        LdapConnectionManager {
            url: self.url.clone(),
            bind_dn: self.bind_dn.clone(),
            bind_password: self.bind_password.clone(),
            conn_settings,
        }
    }
}

pub(super) struct LdapConnectionManager {
    url: Url,
    bind_dn: String,
    bind_password: String,
    conn_settings: LdapConnSettings,
}

impl ManageConnection for LdapConnectionManager {
    type Connection = Ldap;
    type Error = AuthError;

    #[instrument(name = "ldap_connect", skip(self), err)]
    async fn connect(&self) -> Result<Self::Connection, Self::Error> {
        let (conn, mut ldap) =
            LdapConnAsync::from_url_with_settings(self.conn_settings.clone(), &self.url)
                .await
                .into_ldap_error("connection failed")?;

        ldap3::drive!(conn);

        ldap.simple_bind(&self.bind_dn, &self.bind_password)
            .await
            .into_ldap_error("failed to bind with provided bind credentials")?;

        Ok(ldap)
    }

    async fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        if !conn.is_closed() {
            Ok(())
        } else {
            Err(AuthError::NotConnected)
        }
    }

    fn has_broken(&self, conn: &mut Self::Connection) -> bool {
        conn.is_closed()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename = "auth_config")]
pub struct Config {
    /// Configuration for Schlep's connection to the underlying LDAP
    /// authentication directory.
    pub(super) ldap: LdapConfig,
}
