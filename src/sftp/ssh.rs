use std::{
    ffi::OsString,
    future::Future,
    io,
    io::{read_to_string, ErrorKind},
    net::SocketAddr,
    os::unix::prelude::OsStringExt,
    pin::Pin,
    sync::Arc,
};

use ahash::RandomState;
use camino::{Utf8Path, Utf8PathBuf};
use cap_primitives::ambient_authority;
use cap_std::fs_utf8::Dir;
use russh::{
    keys::ssh_key::{self, PrivateKey},
    server::{Auth, Msg, Server, Session},
    Channel,
    ChannelId,
    MethodKind,
    MethodSet,
    Pty,
};
use shlex::bytes::Shlex;
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{event, info, instrument, Level};
use whirlwind::ShardMap;

use super::{hash, Config, Error};
use crate::{auth::AuthClient, sftp::sftp::SftpSession, vfs::VfsSet};

pub type Result<T> = std::result::Result<T, Error>;

pub struct SshServer {
    config: Config,
    methods: MethodSet,
    auth_client: AuthClient,
    vfs_set: VfsSet,
}

impl SshServer {
    pub fn new(config: Config, auth_client: AuthClient, vfs_set: VfsSet) -> Self {
        let mut methods = MethodSet::empty();

        if config.allow_password {
            methods.push(MethodKind::Password);
        }

        if config.allow_publickey {
            methods.push(MethodKind::PublicKey);
        }

        Self {
            config,
            methods,
            auth_client,
            vfs_set,
        }
    }

    pub async fn run(&mut self) -> io::Result<()> {
        let host_keys = get_host_keys(&self.config)?;

        let russh_config = russh::server::Config {
            methods: self.methods.clone(),
            keys: host_keys,
            ..Default::default()
        };

        info!(
            address = &self.config.address,
            port = self.config.port,
            "Listening for SFTP connections"
        );

        self.run_on_address(
            Arc::new(russh_config),
            (self.config.address.clone(), self.config.port),
        )
        .await?;

        Ok(())
    }
}

#[instrument(skip_all, fields(config.private_host_key_dir))]
fn get_host_keys(config: &Config) -> io::Result<Vec<PrivateKey>> {
    let mut keys = Vec::new();
    let key_dir = config.private_host_key_dir.as_path();

    if let Some(key_dir) = Utf8Path::from_path(key_dir) {
        let dir = Dir::open_ambient_dir(key_dir, ambient_authority())?;

        for entry in dir.entries()? {
            let entry = entry?;
            let file_name = entry.file_name()?;

            if file_name.ends_with(".pub") {
                continue;
            }

            let contents = read_to_string(entry.open()?)?;
            if let Ok(private_key) = PrivateKey::from_openssh(contents) {
                let algorithm = private_key.algorithm();
                event!(Level::INFO, %file_name, %algorithm, "imported private key");

                keys.push(private_key);
            }
        }
    }

    Ok(keys)
}

impl Server for SshServer {
    type Handler = SshSession;

    fn new_client(&mut self, sock_addr: Option<SocketAddr>) -> Self::Handler {
        if let Some(sock_addr) = sock_addr {
            event!(Level::INFO, ?sock_addr, "Client connected");
        }

        SshSession::new(
            self.config.clone(),
            self.methods.clone(),
            self.auth_client.clone(),
            self.vfs_set.clone(),
        )
    }

    fn handle_session_error(&mut self, error: Error) {
        match error {
            Error::RusshError(russh::Error::IO(err))
                if err.kind() == ErrorKind::NotConnected
                    || err.kind() == ErrorKind::UnexpectedEof =>
            {
                ()
            }

            _ => event!(
                Level::ERROR,
                err = ?error,
                "Error in session handler"
            ),
        }
    }
}

pub struct SshSession {
    config: Config,
    methods: MethodSet,
    auth_client: AuthClient,
    vfs_set: VfsSet,
    cwd: Utf8PathBuf,
    clients: ShardMap<ChannelId, Channel<Msg>, RandomState>,
}

impl SshSession {
    pub fn new(
        config: Config,
        methods: MethodSet,
        auth_client: AuthClient,
        vfs_set: VfsSet,
    ) -> Self {
        let cwd: Utf8PathBuf = Utf8PathBuf::from("/");

        Self {
            config,
            methods,
            auth_client,
            vfs_set,
            cwd,
            clients: ShardMap::with_hasher(RandomState::default()),
        }
    }

    pub async fn get_channel(&mut self, channel_id: ChannelId) -> Result<Channel<Msg>> {
        if let Some(channel) = self.clients.remove(&channel_id).await {
            Ok(channel)
        } else {
            Err(Error::LostChannel)
        }
    }

    fn exec_command<S>(
        &self,
        stream: S,
        data: &[u8],
    ) -> Option<Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        const MD5SUM: &str = "md5sum";
        const SHA1SUM: &str = "sha1sum";

        let mut shell_parts = Shlex::new(data).map(OsString::from_vec);

        if let Some(command) = shell_parts.next() {
            let vfs_set = self.vfs_set.clone();
            let cwd = self.cwd.clone();
            let arguments = shell_parts.collect::<Vec<_>>();

            if command == MD5SUM {
                return Some(Box::pin(hash::exec_md5sum(vfs_set, cwd, stream, arguments)));
            } else if command == SHA1SUM {
                return Some(Box::pin(hash::exec_sha1sum(
                    vfs_set, cwd, stream, arguments,
                )));
            }
        };

        None
    }
}

impl russh::server::Handler for SshSession {
    type Error = Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth> {
        if self.config.allow_password
            && self
                .auth_client
                .authenticate_password(user, password)
                .await?
        {
            Ok(Auth::Accept)
        } else {
            self.methods.remove(MethodKind::Password);

            Ok(Auth::Reject {
                proceed_with_methods: Some(self.methods.clone()),
            })
        }
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &ssh_key::PublicKey,
    ) -> Result<Auth> {
        if self.config.allow_publickey
            && self
                .auth_client
                .authenticate_public_key(user, public_key)
                .await?
        {
            Ok(Auth::Accept)
        } else {
            self.methods.remove(MethodKind::PublicKey);

            Ok(Auth::Reject {
                proceed_with_methods: Some(self.methods.clone()),
            })
        }
    }

    async fn channel_eof(&mut self, channel: ChannelId, session: &mut Session) -> Result<()> {
        session.close(channel)?;
        self.clients.remove(&channel).await;

        Ok(())
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool> {
        let id = channel.id();
        self.clients.insert(id, channel).await;

        Ok(true)
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<()> {
        session.channel_failure(channel)?;

        Ok(())
    }
    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<()> {
        session.channel_failure(channel)?;

        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel_id: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<()> {
        let channel = self.get_channel(channel_id).await?;
        let channel_stream = channel.into_stream();

        if let Some(result_future) = self.exec_command(channel_stream, data) {
            session.channel_success(channel_id)?;

            if let Ok(_) = result_future.await {
                session.exit_status_request(channel_id, 0)?;
            } else {
                session.exit_status_request(channel_id, 1)?;
            }
        } else {
            session.channel_failure(channel_id)?;
        }

        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<()> {
        if name == "sftp" {
            event!(Level::INFO, ?channel_id, "SFTP session started");
            let channel = self.get_channel(channel_id).await?;
            session.channel_success(channel_id)?;

            let sftp =
                SftpSession::new(self.config.clone(), self.cwd.clone(), self.vfs_set.clone());
            let channel_stream = channel.into_stream();
            russh_sftp::server::run(channel_stream, sftp).await
        } else {
            session.channel_failure(channel_id)?;
        }

        Ok(())
    }
}
