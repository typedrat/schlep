#![forbid(unsafe_code)]
use anyhow::Result;
use camino::Utf8PathBuf;
use mimalloc::MiMalloc;
use tracing_subscriber::{self, fmt::format::FmtSpan};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

pub mod auth;
pub mod config;
mod redis;
pub mod sftp;
pub mod vfs;

use auth::AuthClient;
use config::Config;
use sftp::SshServer;
use vfs::VfsSetBuilder;

pub async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_span_events(FmtSpan::CLOSE)
        .init();

    let config = Config::load()?;

    let redis_pool = if let Some(redis_config) = config.redis {
        Some(redis_config.get_pool().await?)
    } else {
        None
    };

    let auth_client = AuthClient::new(config.auth.clone(), redis_pool.clone()).await?;

    let vfs_builder = VfsSetBuilder::new().local_dir(
        Utf8PathBuf::from("/"),
        Utf8PathBuf::from(config.fs.root_dir.as_str()),
    )?;

    let mut server = SshServer::new(config.sftp.clone(), auth_client, vfs_builder.build());
    server.run().await?;

    Ok(())
}
