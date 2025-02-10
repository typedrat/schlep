use super::sftp::SftpSession;
use crate::config::Config;
use crate::vfs;
use ahash::RandomState;
use anyhow::{bail, Context, Result};
use ldap3::{Ldap, Scope, SearchEntry};
use russh::keys::ssh_key;
use russh::server::{Auth, Msg, Session};
use russh::{Channel, ChannelId};
use scc::hash_map;
use std::net::SocketAddr;
use tracing::{event, Level};

pub struct SshServer {
    config: Config,
    ldap_handle: Ldap,
}

impl SshServer {
    pub fn new(config: Config, ldap_handle: Ldap) -> Self {
        Self {
            config,
            ldap_handle,
        }
    }
}

impl russh::server::Server for SshServer {
    type Handler = SshSession;

    fn new_client(&mut self, sock_addr: Option<SocketAddr>) -> Self::Handler {
        if let Some(sock_addr) = sock_addr {
            event!(Level::INFO, ?sock_addr, "Client connected");
        }

        SshSession::new(self.config.clone(), self.ldap_handle.clone())
    }

    fn handle_session_error(&mut self, error: <Self::Handler as russh::server::Handler>::Error) {
        event!(Level::ERROR, ?error, "Error in session handler");
    }
}

pub struct SshSession {
    config: Config,
    ldap_handle: Ldap,
    clients: hash_map::HashMap<ChannelId, Channel<Msg>, RandomState>,
}

impl SshSession {
    pub fn new(config: Config, ldap_handle: Ldap) -> Self {
        Self {
            config,
            ldap_handle,
            clients: hash_map::HashMap::with_hasher(RandomState::default()),
        }
    }

    pub async fn get_channel(&mut self, channel_id: ChannelId) -> Result<Channel<Msg>> {
        let (_, channel) = self.clients.remove(&channel_id).with_context(|| {
            format!(
                "Failed to retrieve the SSH session channel with ID {}",
                channel_id
            )
        })?;

        Ok(channel)
    }
}

impl russh::server::Handler for SshSession {
    type Error = anyhow::Error;

    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        let mut response = Auth::Reject {
            proceed_with_methods: None,
        };
        let public_key_fingerprint = public_key.fingerprint(Default::default());
        event!(Level::DEBUG, user, ?public_key, "Got public key attempt");

        let search_filter = self
            .config
            .ldap
            .search_filter
            .clone()
            .unwrap_or("(objectClass=user)".to_string());
        let (results, _) = self
            .ldap_handle
            .search(
                &self.config.ldap.base_dn,
                Scope::OneLevel,
                &search_filter,
                vec!["cn", "sshPublicKey"],
            )
            .await?
            .success()?;

        for entry in results {
            let result = SearchEntry::construct(entry);

            let cn = result.attrs.get("cn").cloned().unwrap_or_default();
            let is_user = cn.iter().any(|cn| cn == user);

            let public_key_strings = result
                .attrs
                .get("sshPublicKey")
                .cloned()
                .unwrap_or_default();

            for key_str in public_key_strings {
                let key = ssh_key::PublicKey::from_openssh(&key_str)?;
                let key_fingerprint = key.fingerprint(Default::default());

                if key_fingerprint == public_key_fingerprint && is_user {
                    response = Auth::Accept;
                }
            }
        }

        Ok(response)
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.close(channel)?;
        self.clients.remove(&channel);

        Ok(())
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        _: &mut Session,
    ) -> Result<bool, Self::Error> {
        if let Err((id, _)) = self.clients.insert(channel.id(), channel) {
            bail!("Failed to save channel with ID {}", id);
        }

        Ok(true)
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name == "sftp" {
            event!(Level::INFO, ?channel_id, "SFTP session started");
            let channel = self.get_channel(channel_id).await;
            session.channel_success(channel_id)?;

            let fs_root = vfs::Root::new(self.config.fs.root_dir.as_path())?;
            let sftp = SftpSession::new(self.config.clone(), fs_root);
            let channel_stream = channel?.into_stream();
            russh_sftp::server::run(channel_stream, sftp).await
        } else {
            session.channel_failure(channel_id)?;
        }

        Ok(())
    }
}
