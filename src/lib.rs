#![forbid(unsafe_code)]
use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use ldap3::{LdapConnAsync, LdapConnSettings};
use mimalloc::MiMalloc;
use rand::rngs::OsRng;
use russh::{
    keys::{ssh_key, PrivateKey},
    server::Server,
    MethodKind,
    MethodSet,
};
use tracing::{info, warn, Level};
use tracing_subscriber;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

pub mod config;
pub mod server;
pub mod vfs;

use config::Config;
use server::SshServer;

use crate::vfs::VfsSetBuilder;

fn load_host_keys(key_paths: &[PathBuf]) -> Result<Vec<PrivateKey>> {
    if key_paths.is_empty() {
        let random_host_key = PrivateKey::random(&mut OsRng, ssh_key::Algorithm::Ed25519)?;
        warn!(
            "No host keys provided, generating random key: {}",
            random_host_key.public_key().to_openssh()?
        );

        Ok(vec![random_host_key])
    } else {
        let mut host_keys = Vec::with_capacity(key_paths.len());

        for key_path in key_paths {
            let host_key = PrivateKey::read_openssh_file(key_path)?;
            info!("Loaded host key: {}", host_key.public_key().to_openssh()?);
            host_keys.push(host_key);
        }

        Ok(host_keys)
    }
}

pub async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .init();

    let config = Config::load()?;

    let ldap_settings = LdapConnSettings::new().set_no_tls_verify(config.ldap.tls_no_verify);
    let (ldap_conn, mut ldap_handle) =
        LdapConnAsync::with_settings(ldap_settings, &config.ldap.url)
            .await
            .context(format!(
                "Failed to connect to LDAP server {}",
                &config.ldap.url
            ))?;
    ldap3::drive!(ldap_conn);

    ldap_handle
        .simple_bind(&config.ldap.bind_user, &config.ldap.bind_password)
        .await
        .context("Failed to bind LDAP user")?;

    let mut methods = MethodSet::empty();
    methods.push(MethodKind::PublicKey);
    let russh_config = russh::server::Config {
        methods,
        keys: load_host_keys(&config.sftp.private_host_keys)?,
        ..Default::default()
    };

    let vfs_builder = VfsSetBuilder::new().local_dir(
        Utf8PathBuf::from("/"),
        Utf8PathBuf::from(config.fs.root_dir.as_str()),
    )?;

    let mut server = SshServer::new(config.clone(), ldap_handle.clone(), vfs_builder.build());

    info!(
        address = config.sftp.address,
        port = config.sftp.port,
        "Listening for SFTP connections"
    );
    server
        .run_on_address(
            Arc::new(russh_config),
            (config.sftp.address, config.sftp.port),
        )
        .await?;

    Ok(())
}
