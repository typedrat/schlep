#![forbid(unsafe_code)]
use anyhow::Result;
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

    let vfs_builder = VfsSetBuilder::from_config(config.fs)?;

    let mut server = SshServer::new(config.sftp.clone(), auth_client, vfs_builder.build());
    server.run().await?;

    Ok(())
}
